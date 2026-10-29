//! The Map explorer in the tray service: what this member added (`/maps/mine`) and uploads, which keep going when the
//! window closes. Pictures are sent as the original files (Orbiters recognises them by their hash and re-encodes them
//! without their metadata); a 3D preview is the session's splats as a web `.splat` (orbiters_reconstruct::web),
//! starting from the camera the desktop's 3D view opens on.

use super::{Controller, NoticeKind, Shared};
use crate::ipc::ToClient;
use crate::state::{MapsMine, UploadKind, UploadState, UploadStatus};
use orbiters_core::maps::{Part, PreviewFiles};
use orbiters_core::{bundle, reconstruction, visited};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

/// What Orbiters takes per picture, and per request.
const MAX_PICTURE_BYTES: u64 = 25 << 20;
const PICTURES_PER_REQUEST: usize = 12;
/// Finished uploads kept listed.
const KEEP_FINISHED: usize = 12;

fn iso(unix: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix as i64).ok().and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()).unwrap_or_default()
}

impl Controller {
    /// What this member added to the Map explorer, as last fetched.
    pub fn maps(&self) -> Option<MapsMine> {
        self.maps_mine.lock().unwrap().clone()
    }

    /// Replaces what this member added (screenshots).
    pub fn set_maps(&self, mine: MapsMine) {
        *self.maps_mine.lock().unwrap() = Some(mine);
    }

    /// Fetches what this member added (signed out: nothing), then tells every interface.
    pub fn refresh_maps(self: &Shared) {
        let c = self.clone();
        std::thread::spawn(move || {
            let mine = match c.account_api() {
                None => MapsMine::default(),
                Some((api, token)) => match api.mine(&token) {
                    Ok(m) => MapsMine { worlds: m.worlds, error: None },
                    Err(e) => MapsMine { worlds: c.maps().map(|m| m.worlds).unwrap_or_default(), error: Some(e.to_string()) },
                },
            };
            *c.maps_mine.lock().unwrap() = Some(mine.clone());
            c.broadcast(|| ToClient::Maps { mine: mine.clone() });
            if let Some(ctx) = c.repaint.lock().unwrap().as_ref() {
                ctx.request_repaint();
            }
        });
    }

    fn start_upload(&self, world_id: &str, world_name: &str, kind: UploadKind, paths: Vec<PathBuf>) -> anyhow::Result<u64> {
        anyhow::ensure!(orbiters_core::vrchat_log::is_world_id(world_id), "Not a VRChat world.");
        let id = self.upload_serial.fetch_add(1, Ordering::Relaxed) + 1;
        self.update(|s| {
            let running = s.uploads.iter().any(|u| u.running() && u.world_id == world_id && u.kind == kind);
            if !running {
                let state = if kind == UploadKind::Preview { UploadState::Preparing } else { UploadState::Sending };
                s.uploads.push(UploadStatus { id, world_id: world_id.into(), world_name: world_name.into(), kind, paths, sent: 0, total: 0, state });
                let finished: Vec<u64> = s.uploads.iter().filter(|u| !u.running()).map(|u| u.id).collect();
                if finished.len() > KEEP_FINISHED {
                    let drop = &finished[..finished.len() - KEEP_FINISHED];
                    s.uploads.retain(|u| !drop.contains(&u.id));
                }
            }
            if running {
                Err(anyhow::anyhow!("This is already being sent."))
            } else {
                Ok(id)
            }
        })
    }

    /// Reports the bytes sent, at most every 100 ms (each change goes to every interface).
    fn progress(self: &Shared, id: u64) -> impl FnMut(u64, u64) + Send + 'static {
        let c = self.clone();
        let mut last = Instant::now() - std::time::Duration::from_secs(1);
        move |sent, total| {
            if sent < total && last.elapsed().as_millis() < 100 {
                return;
            }
            last = Instant::now();
            c.update_if(|s| match s.uploads.iter_mut().find(|u| u.id == id) {
                Some(u) => {
                    u.state = UploadState::Sending;
                    u.sent = sent;
                    u.total = total;
                    true
                }
                None => false,
            });
        }
    }

    fn finish_upload(self: &Shared, id: u64, state: UploadState) {
        let finished = self.update(|s| {
            let u = s.uploads.iter_mut().find(|u| u.id == id)?;
            u.state = state.clone();
            Some((u.world_name.clone(), u.kind))
        });
        if let Some((name, kind)) = finished {
            let (tone, message) = match (&state, kind) {
                (UploadState::Done { .. }, UploadKind::Preview) => (NoticeKind::Success, format!("3D preview of {name} shared on the Map explorer.")),
                (UploadState::Done { added: 0, .. }, UploadKind::Pictures) => {
                    (NoticeKind::Info, format!("These pictures of {name} were already on the Map explorer."))
                }
                (UploadState::Done { added, skipped }, UploadKind::Pictures) => {
                    let more = if *skipped > 0 { format!(" ({skipped} already there)") } else { String::new() };
                    (NoticeKind::Success, format!("{added} picture{} of {name} shared on the Map explorer{more}.", if *added == 1 { "" } else { "s" }))
                }
                (UploadState::Failed { message }, _) => (NoticeKind::Error, format!("Could not share to the Map explorer: {message}")),
                _ => (NoticeKind::Info, String::new()),
            };
            if !message.is_empty() {
                self.notify(tone, message);
            }
        }
        self.refresh_maps();
    }

    /// Shares pictures of a world: VRChat screenshots, sent as they are, 12 per request.
    pub fn upload_pictures(self: &Shared, world_id: String, world_name: String, paths: Vec<PathBuf>) -> anyhow::Result<()> {
        let Some((api, token)) = self.account_api() else { anyhow::bail!("Sign in to Orbiters first.") };
        anyhow::ensure!(!paths.is_empty(), "Choose pictures to share.");
        let id = self.start_upload(&world_id, &world_name, UploadKind::Pictures, paths.clone())?;
        let c = self.clone();
        std::thread::spawn(move || {
            let state = match c.send_pictures(&api, &token, id, &world_id, &paths) {
                Ok((added, skipped)) => UploadState::Done { added, skipped },
                Err(e) => UploadState::Failed { message: e.to_string() },
            };
            c.finish_upload(id, state);
        });
        Ok(())
    }

    fn send_pictures(
        self: &Shared,
        api: &orbiters_core::account::Api,
        token: &str,
        id: u64,
        world_id: &str,
        paths: &[PathBuf],
    ) -> anyhow::Result<(usize, usize)> {
        let sizes: Vec<u64> = paths.iter().map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)).collect();
        if let Some(i) = sizes.iter().position(|s| *s > MAX_PICTURE_BYTES) {
            anyhow::bail!("{} is over 25 MB.", paths[i].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
        let total: u64 = sizes.iter().sum();
        let (mut added, mut skipped, mut done) = (0, 0, 0u64);
        for (chunk, chunk_sizes) in paths.chunks(PICTURES_PER_REQUEST).zip(sizes.chunks(PICTURES_PER_REQUEST)) {
            let mut parts = Vec::with_capacity(chunk.len());
            let mut taken = Vec::with_capacity(chunk.len());
            for p in chunk {
                taken.push(visited::read_shot(p).map(|i| iso(i.taken)).unwrap_or_default());
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "picture.png".into());
                // Read from the file while it is sent: twelve pictures never sit in memory at once.
                parts.push(Part::file("pictures", name, "image/png", p)?);
            }
            let chunk_bytes: u64 = chunk_sizes.iter().sum();
            let mut report = self.progress(id);
            let base = done;
            let r = api.add_pictures(token, world_id, parts, &taken, move |sent, all| {
                // The request's own bytes, scaled to these pictures' share of the whole upload.
                let k = if all == 0 { 0.0 } else { sent as f64 / all as f64 };
                report(base + (k * chunk_bytes as f64) as u64, total);
            })?;
            done += chunk_bytes;
            added += r.added;
            skipped += r.skipped;
        }
        Ok((added, skipped))
    }

    /// Shares a session's reconstruction as the world's 3D preview (replacing this member's earlier one).
    pub fn upload_preview(self: &Shared, world_id: String, world_name: String, session: PathBuf) -> anyhow::Result<()> {
        let Some((api, token)) = self.account_api() else { anyhow::bail!("Sign in to Orbiters first.") };
        anyhow::ensure!(reconstruction::read_report(&session).is_some(), "Reconstruct this session first.");
        let id = self.start_upload(&world_id, &world_name, UploadKind::Preview, vec![session.clone()])?;
        let c = self.clone();
        std::thread::spawn(move || {
            let state = match web_preview(&session).and_then(|(files, sky, metadata)| {
                let total = files.len() + sky.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
                c.update_if(|s| match s.uploads.iter_mut().find(|u| u.id == id) {
                    Some(u) => {
                        u.state = UploadState::Sending;
                        u.total = total;
                        true
                    }
                    None => false,
                });
                api.add_preview(&token, &world_id, files, sky.as_deref(), &metadata, c.progress(id)).map(|_| ())
            }) {
                Ok(()) => UploadState::Done { added: 1, skipped: 0 },
                Err(e) => UploadState::Failed { message: e.to_string() },
            };
            c.finish_upload(id, state);
        });
        Ok(())
    }
}

/// A session's splats as a web `.splat` and as VRChat's splat trees (their merged boxes tuned on the GPU when there is
/// one), its sky map (sky.png as trained, when training learnt one) and the upload's metadata: the start view
/// (right-handed like the files: Unity's Z negated), the held-out quality and when the session was recorded.
pub(super) fn web_preview(session: &Path) -> anyhow::Result<(PreviewFiles, Option<PathBuf>, serde_json::Value)> {
    use orbiters_reconstruct::timing::PrepStage;
    let _total = PrepStage::new("total");
    let stage = PrepStage::new("read result");
    let result = reconstruction::result_dir(session);
    let g = orbiters_reconstruct::ply::read(&result.join(reconstruction::PLY_FILE))?;
    drop(stage);
    let stage = PrepStage::new("start view");
    let (views, start) = crate::views::recorded(session)?;
    let v = views[start];
    let depth = crate::views::depth_ahead(&g, &v);
    let target = [v.position[0] + v.forward[0] * depth, v.position[1] + v.forward[1] * depth, v.position[2] + v.forward[2] * depth];
    drop(stage);
    let stage = PrepStage::new("web splat");
    let web = orbiters_reconstruct::web::splat_file(&g, orbiters_reconstruct::web::DEFAULT_MAX, v.position);
    drop(stage);
    anyhow::ensure!(web.count >= 1000, "This reconstruction has too few splats to show the world.");
    let stage = PrepStage::new("session cameras");
    let cameras = orbiters_reconstruct::tree_tune::session_cameras(session)?;
    drop(stage);
    let stage = PrepStage::new("gpu setup");
    let mut gpu = orbiters_reconstruct::gpu::Gpu::with_options(orbiters_reconstruct::gpu::DeviceOptions {
        profile: std::env::var_os("ORBITERS_PREP_PROFILE").is_some(),
        ..Default::default()
    })
    .ok();
    if std::env::var_os("ORBITERS_PREP_TIMING").is_some() {
        eprintln!("upload preparation adapter: {}", gpu.as_ref().map_or("CPU (untuned)", |g| g.adapter.as_str()));
    }
    drop(stage);
    // Names this upload in its three trees, so a world can tell when a slot's files have been replaced.
    let key = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u32) ^ web.count as u32;
    let trees = orbiters_reconstruct::tree_tune::vrchat_files(&g, &cameras, gpu.as_mut(), key, |_| {})?;
    if let Some(gpu) = &gpu {
        if let Some(p) = &gpu.profiler {
            eprintln!("upload preparation GPU span: {:?}; passes: {:?}", p.span.get(), p.totals.borrow());
        }
    }
    let files = PreviewFiles { splat: web.bytes, vrchat_pc: trees.pc, vrchat_quest: trees.quest, vrchat_coarse: trees.coarse };
    let flip = |p: [f32; 3]| [p[0], p[1], -p[2]];
    let report = reconstruction::read_report(session);
    let recorded = bundle::read_manifest(session).map(|m| m.created_utc).ok();
    let metadata = serde_json::json!({
        "view": { "position": flip(v.position), "target": flip(target) },
        "quality": report.and_then(|r| r.psnr_held_out),
        "capturedAt": recorded,
    });
    let stage = PrepStage::new("sky");
    let sky = Some(result.join(reconstruction::SKY_FILE)).filter(|p| p.is_file());
    drop(stage);
    Ok((files, sky, metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Local preparation only; never contacts a server. Run in release mode, one GPU test at a time.
    #[test]
    #[ignore]
    fn upload_prep_timings() {
        let sessions = std::env::var("ORBITERS_TEST_SESSION").expect("ORBITERS_TEST_SESSION (semicolon-separated folders)");
        for (index, session) in sessions.split(';').filter(|s| !s.is_empty()).enumerate() {
            eprintln!("upload preparation session {index}: {session}");
            let (files, sky, metadata) = web_preview(Path::new(session)).expect("prepare");
            eprintln!("upload preparation bytes: {}; sky: {}; metadata: {metadata}", files.len(), sky.is_some());
            if let Some(out) = std::env::var_os("ORBITERS_PREP_OUT") {
                let out = PathBuf::from(out).join(index.to_string());
                std::fs::create_dir_all(&out).unwrap();
                for (name, bytes) in
                    [("web.splat", &files.splat), ("pc.tree", &files.vrchat_pc), ("quest.tree", &files.vrchat_quest), ("coarse.tree", &files.vrchat_coarse)]
                {
                    std::fs::write(out.join(name), bytes).unwrap();
                }
                std::fs::write(out.join("metadata.json"), serde_json::to_vec_pretty(&metadata).unwrap()).unwrap();
            }
        }
    }

    /// A reconstructed session shared in 3D on a disposable server: `ORBITERS_TEST_API=http://127.0.0.1:4230
    /// ORBITERS_TEST_TOKEN=orbit-… ORBITERS_TEST_WORLD=wrld_… ORBITERS_TEST_SESSION=<session folder>
    /// cargo test -p orbiters-desktop shares_a_session_in_3d -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn shares_a_session_in_3d() {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
        let api = orbiters_core::account::Api::at(&env("ORBITERS_TEST_API"));
        let world = env("ORBITERS_TEST_WORLD");
        let t = Instant::now();
        let (files, sky, metadata) = web_preview(Path::new(&env("ORBITERS_TEST_SESSION"))).unwrap();
        println!("{:.1} MB web preview and VRChat trees in {:?}: {metadata}", files.len() as f64 / 1e6, t.elapsed());
        assert_eq!(files.splat.len() % orbiters_reconstruct::web::SPLAT_BYTES, 0);
        assert_eq!(&files.vrchat_pc[..8], orbiters_reconstruct::tree::MAGIC);
        let view = &metadata["view"];
        assert_eq!(view["position"].as_array().unwrap().len(), 3);
        let before = api.world(&world).unwrap().unwrap().previews;
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let s = sent.clone();
        let total = files.len();
        api.add_preview(&env("ORBITERS_TEST_TOKEN"), &world, files, sky.as_deref(), &metadata, move |n, _| s.store(n, Ordering::Relaxed)).unwrap();
        assert!(sent.load(Ordering::Relaxed) >= total);
        let page = api.world(&world).unwrap().unwrap();
        println!("previews {before} → {}", page.previews);
        assert!(page.previews >= before.max(1));
    }

    /// Shares sessions in 3D with the account signed in on this PC, on Orbiters itself, as **Share in 3D** does:
    /// `ORBITERS_SHARE="<world id>|<session folder>;…" cargo test -p orbiters-desktop shares_sessions_with_the_signed_in_account
    /// -- --ignored --nocapture` (the credential is read like the app reads it, never printed).
    #[test]
    #[ignore]
    fn shares_sessions_with_the_signed_in_account() {
        use orbiters_core::account::{self, Api, Server};
        let account = account::read(Server::Production).expect("sign in to Orbiters first");
        let api = Api::new(Server::Production);
        for item in std::env::var("ORBITERS_SHARE").expect("ORBITERS_SHARE").split(';').filter(|s| !s.is_empty()) {
            let (world, session) = item.split_once('|').expect("<world id>|<session folder>");
            let t = Instant::now();
            let (files, sky, metadata) = web_preview(Path::new(session)).unwrap();
            println!("{world}: {:.1} MB in {:.1} s, sky {}", files.len() as f64 / 1e6, t.elapsed().as_secs_f32(), sky.is_some());
            api.add_preview(&account.token, world, files, sky.as_deref(), &metadata, |_, _| {}).unwrap();
            println!("{world}: shared");
        }
    }
}
