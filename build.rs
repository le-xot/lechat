fn main() {
    println!("cargo:rerun-if-changed=assets/lechat.ico");

    // Windows Explorer, the task bar and the title bar read the icon from the
    // executable's resource section, so it has to be linked in at build time.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/lechat.ico");
    res.set("OriginalFilename", "lechat.exe");
    res.set("LegalCopyright", "MIT");
    res.compile().expect("failed to embed Windows resources");
}
