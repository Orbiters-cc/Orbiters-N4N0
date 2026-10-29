//! Read-only machine audit. Writes only Orbiters' discovery cache, never VCC settings or projects.
fn main() -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    eprintln!("Reading shared VCC settings and database…");
    let registered = orbiters_projects::runtime()?.block_on(orbiters_projects::vpm::registered(&orbiters_projects::vcc_dir()))?;
    eprintln!("Shared store: {} registered projects. Starting discovery…", registered.len());
    let scan = orbiters_projects::discovery::discover(orbiters_projects::discovery::cached(), true, &cancel, |s| {
        eprintln!(
            "Discovery: {} registered, {} proposals - {}",
            s.projects.iter().filter(|p| p.registered).count(),
            s.projects.iter().filter(|p| !p.registered).count(),
            s.status
        )
    })?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"registered":scan.projects.iter().filter(|p|p.registered).count(),"proposals":scan.projects.iter().filter(|p|!p.registered).count(),"avatars":scan.projects.iter().map(|p|p.avatars.len()).sum::<usize>(),"fullScanMs":scan.full_scan_ms,"directories":scan.directories,"elapsedMs":started.elapsed().as_millis(),"warnings":scan.warnings})
        )?
    );
    Ok(())
}
