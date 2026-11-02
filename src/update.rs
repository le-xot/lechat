use std::path::Path;

use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    Disabled,
    Checking,
    UpToDate,
    /// New binary is in place; restart to use it.
    Ready(String),
    /// New version exists but this install is not writable (package manager).
    Available(String),
    Error(String),
}

/// Updates are skipped in development builds, when the binary lives inside
/// `target/`, or when explicitly disabled through the environment.
pub fn enabled_for(exe: &Path, debug_build: bool) -> bool {
    if debug_build || std::env::var_os("LECHAT_NO_UPDATE").is_some() {
        return false;
    }
    let path = exe.to_string_lossy();
    !(path.contains("/target/release/")
        || path.contains("/target/debug/")
        || path.contains("\\target\\release\\")
        || path.contains("\\target\\debug\\"))
}

/// Checks whether the binary can be replaced in place.
pub fn probe_writable(exe: &Path) -> bool {
    let Some(dir) = exe.parent() else {
        return false;
    };
    let probe = dir.join(".lechat-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn check(tx: UnboundedSender<UpdateState>) {
    let exe = std::env::current_exe().unwrap_or_default();
    if !enabled_for(&exe, cfg!(debug_assertions)) {
        let _ = tx.send(UpdateState::Disabled);
        return;
    }

    let _ = tx.send(UpdateState::Checking);
    let built = self_update::backends::github::Update::configure()
        .repo_owner("le-xot")
        .repo_name("lechat")
        .bin_name("lechat")
        .current_version(env!("CARGO_PKG_VERSION"))
        .build();
    let updater = match built {
        Ok(updater) => updater,
        Err(err) => {
            let _ = tx.send(UpdateState::Error(format!("{err:#}")));
            return;
        }
    };

    if !probe_writable(&exe) {
        let state = match updater.is_update_available() {
            Ok(Some(release)) => UpdateState::Available(release.version().to_string()),
            Ok(None) => UpdateState::UpToDate,
            Err(err) => UpdateState::Error(format!("{err:#}")),
        };
        let _ = tx.send(state);
        return;
    }

    let state = match updater.update() {
        Ok(self_update::VersionStatus::UpToDate(_)) => UpdateState::UpToDate,
        Ok(self_update::VersionStatus::Updated(version)) => UpdateState::Ready(version),
        Ok(_) => UpdateState::UpToDate,
        Err(err) => UpdateState::Error(format!("{err:#}")),
    };
    let _ = tx.send(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_enabled_only_for_user_install() {
        assert!(enabled_for(
            Path::new("/home/u/.local/bin/lechat"),
            false
        ));
        assert!(!enabled_for(
            Path::new("/repo/target/release/lechat"),
            false
        ));
        assert!(!enabled_for(
            Path::new("/home/u/.local/bin/lechat"),
            true
        ));
        assert!(enabled_for(Path::new("/usr/bin/lechat"), false));
    }

    #[cfg(unix)]
    #[test]
    fn writability_probe() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("lechat");
        std::fs::write(&exe, b"x").unwrap();
        assert!(probe_writable(&exe));

        let ro = dir.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        let mut perms = std::fs::metadata(&ro).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o555);
        std::fs::set_permissions(&ro, perms).unwrap();
        assert!(!probe_writable(&ro.join("lechat")));
    }
}
