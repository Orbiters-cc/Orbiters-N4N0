//! Embeds every `catalog/<area>/<language>.json`: an area is added by adding its folder, so areas translated side by side never edit the
//! same file, this one included.

use std::path::Path;

fn main() {
    let root = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("catalog");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut areas: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    areas.sort();
    let mut out = String::from("&[\n");
    for area in areas {
        println!("cargo:rerun-if-changed={}", area.display());
        for language in ["ja", "fr", "es", "pt-BR"] {
            let file = area.join(format!("{language}.json"));
            if file.is_file() {
                println!("cargo:rerun-if-changed={}", file.display());
                let name = area.file_name().unwrap().to_string_lossy();
                out.push_str(&format!("    ({name:?}, {language:?}, include_str!({:?})),\n", file.display().to_string()));
            }
        }
    }
    out.push_str("]\n");
    let to = Path::new(&std::env::var("OUT_DIR").unwrap()).join("catalogs.rs");
    std::fs::write(to, out).unwrap();
}
