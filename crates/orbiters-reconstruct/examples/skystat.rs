//! What seeding a sky map finds in a session (crate::sky::seed_map), without training: sky pixels, texels seen, and
//! the parallax test's cells.
//!
//! ```text
//! cargo run --release --example skystat -- <session> [--width N]
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::{prepare, sky, train};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut session, mut width) = (None, 1920u32);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--width" => width = it.next().context("--width N")?.parse()?,
            _ => session = Some(PathBuf::from(a)),
        }
    }
    let session = session.context("usage: skystat <session> [--width N]")?;
    let prepared = prepare::prepare(&session, &prepare::Options { width, ..Default::default() }, |_| {})?;
    let data = &prepared.data;
    let held_out = train::held_out_views(data, train::Settings::default().hold_out_every);
    let (_, found) = sky::seed_map(data, 2048, &held_out);
    println!(
        "{}: {} sky pixels, {} texels seen; {} of the {} cells seen from both halves of the walk differ between them",
        session.display(),
        found.pixels,
        found.texels_seen,
        found.cells_parallax,
        found.cells_tested
    );
    Ok(())
}
