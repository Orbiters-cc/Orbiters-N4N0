//! `--only worlds-overlay`: the SteamVR overlay's 3D worlds panel with stand-in captures, painted on the CPU as SteamVR
//! gets it (overlay/worlds/panel/shots.rs): every page (the results ordered and filtered, the filter, a capture's page
//! with its feedback, labels, a reason, comments, a comment, pictures), the capture's bar alone in its states, and
//! `with-bar-detail.png` / `with-bar-browse.png`: the bar under the panel with the capture's page open and closed.

pub(super) fn shots(out: &std::path::Path) -> anyhow::Result<()> {
    crate::overlay::worlds::panel::shots(out)?;
    let mut names: Vec<String> = std::fs::read_dir(out)?
        .flatten()
        .map(|f| f.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".png") && (n.starts_with("panel-") || n.starts_with("bar-") || n.starts_with("with-bar-")))
        .collect();
    names.sort();
    for n in names {
        println!("{}", out.join(n).display());
    }
    Ok(())
}
