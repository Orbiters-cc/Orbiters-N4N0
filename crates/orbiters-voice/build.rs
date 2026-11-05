//! The worker finds its shared libraries beside itself on macOS: in the target folder and in the app's bundle alike.
//! The prebuilt libraries are named through `@rpath`, and nothing else gives the program one. (On Windows the system
//! looks beside the program by itself.)

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@loader_path");
    }
}
