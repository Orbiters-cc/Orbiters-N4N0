fn main() {
    // The executable's icon (Explorer, taskbar pins).
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Orbiters desktop");
        res.set("FileDescription", "Orbiters desktop");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the icon: {e}");
        }
    }
}
