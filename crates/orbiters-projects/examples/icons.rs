//! The icons of the packages the shared VPM repositories list (or the first N: `icons 40`), as the Unity projects page
//! finds them: one line per package (where its icon came from), and a contact sheet of them all (`icons 40 sheet.png`).
//! Reads the repositories' caches only; writes Orbiters' icon cache.
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let limit = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(usize::MAX);
    let started = std::time::Instant::now();
    let view = orbiters_projects::runtime()?.block_on(orbiters_projects::vpm::view(&orbiters_projects::vcc_dir(), None, false))?;
    let packages: Vec<_> = view.packages.into_iter().take(limit).collect();
    let sources: Vec<_> = packages.iter().map(orbiters_projects::icons::sources).collect();
    let icons = orbiters_projects::icons::resolve_all(&sources);
    let cell = orbiters_projects::icons::SIDE + 8;
    let columns = 10u32;
    let mut sheet = image::RgbaImage::from_pixel(columns * cell, (packages.len() as u32).div_ceil(columns) * cell, image::Rgba([30, 30, 34, 255]));
    for (i, ((p, s), icon)) in packages.iter().zip(&sources).zip(&icons).enumerate() {
        let from = match (icon, &s.logo) {
            (None, _) => "none".to_string(),
            (Some(_), Some(logo)) => format!("Orbiters logo {logo}"),
            (Some(_), None) => format!("site {}", s.sites.join(" | ")),
        };
        println!("{:<48} {}", p.name, from);
        if let Some(icon) = icon {
            let image = image::RgbaImage::from_raw(icon.size[0], icon.size[1], icon.rgba.clone()).unwrap();
            let (x, y) = ((i as u32 % columns) * cell + 4, (i as u32 / columns) * cell + 4);
            image::imageops::overlay(&mut sheet, &image, x as i64, y as i64);
        }
    }
    if let Some(path) = args.get(2) {
        sheet.save(path)?;
    }
    eprintln!("{} packages, {} icons in {:.1} s", packages.len(), icons.iter().flatten().count(), started.elapsed().as_secs_f32());
    Ok(())
}
