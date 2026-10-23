//! The setup's icon and manifest, and the programs it installs when `ORBITERS_SETUP_PAYLOAD` names a payload
//! (`orbiters_update::payload`). `cargo xtask dist` builds it twice: without (the uninstaller left in the install
//! folder, `orbiters-setup.exe`), then with that one inside the payload (the setup people download).

/// Runs as the person who started it: Windows would otherwise ask for administrator rights for a program whose name
/// says "setup" (its installer detection), and a per-user install needs none. Per-monitor DPI, Windows 10 and 11.
#[cfg(windows)]
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(payload)");
    println!("cargo:rerun-if-env-changed=ORBITERS_SETUP_PAYLOAD");
    if let Some(path) = std::env::var_os("ORBITERS_SETUP_PAYLOAD").filter(|p| !p.is_empty()) {
        let path = std::path::absolute(std::path::PathBuf::from(path)).expect("ORBITERS_SETUP_PAYLOAD: not a path");
        assert!(path.is_file(), "ORBITERS_SETUP_PAYLOAD: no file at {}", path.display());
        println!("cargo:rerun-if-changed={}", path.display());
        println!("cargo:rustc-cfg=payload");
        println!("cargo:rustc-env=ORBITERS_SETUP_PAYLOAD_FILE={}", path.display());
    }

    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../orbiters-desktop/assets/icon.ico");
        res.set("ProductName", "Orbiters desktop");
        res.set("FileDescription", "Orbiters desktop setup");
        res.set("CompanyName", "Orbiters");
        res.set("LegalCopyright", "Copyright (c) 2026 Enzo DUTRA / blackorbit");
        res.set_manifest(MANIFEST);
        if let Err(e) = res.compile() {
            // A release without its manifest would ask for administrator rights: not one to ship.
            if std::env::var("PROFILE").as_deref() == Ok("release") {
                panic!("could not embed the setup's icon and manifest: {e}");
            }
            println!("cargo:warning=could not embed the icon and manifest: {e}");
        }
    }
}
