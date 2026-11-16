fn main() {
    // The executable's icon (Explorer, taskbar pins).
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Orbiters - N4N0");
        res.set("FileDescription", "Orbiters - N4N0");
        // The release's version (`.cargo/config.toml` [env], from `cargo xtask bump`), not the crate's: `dist` checks it.
        let version = std::env::var("ORBITERS_VERSION").expect("ORBITERS_VERSION is set in .cargo/config.toml");
        println!("cargo:rerun-if-env-changed=ORBITERS_VERSION");
        res.set("ProductVersion", &version);
        res.set("FileVersion", &version);
        let number = |i: usize| version.split(['.', '-', '+']).nth(i).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
        let packed = number(0) << 48 | number(1) << 32 | number(2) << 16;
        res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);
        res.set_version_info(winresource::VersionInfo::FILEVERSION, packed);
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the icon: {e}");
        }
    }
}
