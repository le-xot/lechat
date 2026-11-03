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

fn log(text: &str) {
    let Some(dirs) = directories::ProjectDirs::from("dev", "le-xot", "lechat") else {
        return;
    };
    let dir = dirs.data_dir();
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("update.log"))
    {
        use std::io::Write;
        let _ = writeln!(file, "{} {text}", chrono::Utc::now().to_rfc3339());
    }
}

fn send(tx: &UnboundedSender<UpdateState>, state: UpdateState) {
    log(&format!("{state:?}"));
    let _ = tx.send(state);
}

pub fn update_log_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "le-xot", "lechat")
        .map(|dirs| dirs.data_dir().join("update.log"))
}

pub fn check(tx: UnboundedSender<UpdateState>) {
    let exe = std::env::current_exe().unwrap_or_default();
    let writable = probe_writable(&exe);
    log(&format!(
        "check: version={} exe={} writable={writable}",
        env!("CARGO_PKG_VERSION"),
        exe.display()
    ));
    if !enabled_for(&exe, cfg!(debug_assertions)) {
        send(&tx, UpdateState::Disabled);
        return;
    }

    send(&tx, UpdateState::Checking);
    let built = self_update::backends::github::Update::configure()
        .repo_owner("le-xot")
        .repo_name("lechat")
        .bin_name("lechat")
        .current_version(env!("CARGO_PKG_VERSION"))
        .build();
    let updater = match built {
        Ok(updater) => updater,
        Err(err) => {
            send(&tx, UpdateState::Error(format!("{err:#}")));
            return;
        }
    };

    if !writable {
        let state = match updater.is_update_available() {
            Ok(Some(release)) => UpdateState::Available(release.version().to_string()),
            Ok(None) => UpdateState::UpToDate,
            Err(err) => UpdateState::Error(format!("{err:#}")),
        };
        send(&tx, state);
        return;
    }

    let state = match updater.update() {
        Ok(self_update::VersionStatus::UpToDate(_)) => UpdateState::UpToDate,
        Ok(self_update::VersionStatus::Updated(version)) => UpdateState::Ready(version),
        Ok(_) => UpdateState::UpToDate,
        Err(err) => UpdateState::Error(format!("{err:#}")),
    };
    send(&tx, state);
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

#[cfg(test)]
mod live_tests {
    #[test]
    #[ignore = "живой запрос к GitHub Releases"]
    fn github_sees_windows_asset_from_older_version() {
        let updater = self_update::backends::github::Update::configure()
            .repo_owner("le-xot")
            .repo_name("lechat")
            .bin_name("lechat")
            .current_version("0.0.1")
            .target("x86_64-pc-windows-msvc")
            .build()
            .unwrap();
        let releases = updater.get_newer_releases().unwrap();
        let release = releases
            .into_vec()
            .into_iter()
            .next()
            .expect("нет новых релизов");
        let asset = release
            .asset_for("x86_64-pc-windows-msvc", None)
            .expect("нет windows-ассета");
        println!("release={} asset={}", release.version(), asset.name());
        assert!(asset.name().contains("x86_64-pc-windows-msvc"));
    }
}
