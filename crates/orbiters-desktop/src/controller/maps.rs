//! The Map explorer in the tray service: what this member added (`/maps/mine`) and uploads, which keep going when the
//! window closes. Pictures are sent as the original files (Orbiters recognises them by their hash and re-encodes them
//! without their metadata); a 3D preview is the session's splats as VRChat's splat trees (orbiters_reconstruct::tree),
//! the PC one also the website's `.splat` (Orbiters makes it from its leaves), starting from the view the desktop's 3D
//! view opens on.

use super::{Controller, NoticeKind, Shared};
use crate::ipc::ToClient;
use crate::state::{MapsMine, UploadKind, UploadState, UploadStatus};
use orbiters_core::maps::{Part, PreviewFiles};
use orbiters_core::{bundle, reconstruction, visited};
use orbiters_i18n::tr;
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
        let identity = self.maps_identity();
        self.maps_mine.lock().unwrap().as_ref().filter(|(of, _)| *of == identity).map(|(_, mine)| mine.clone())
    }

    /// Replaces what this member added (screenshots).
    pub fn set_maps(&self, mine: MapsMine) {
        *self.maps_mine.lock().unwrap() = Some((self.maps_identity(), mine));
    }

    /// Whose "My maps" a list is: the server and credential signed in now (`None` signed out).
    fn maps_identity(&self) -> Option<String> {
        self.account_api().map(|(api, token)| format!("{}|{token}", api.url("")))
    }

    /// Keeps a fetched list when it is the answer of the newest fetch (`turn`) and of the account still signed in: an
    /// older, slower answer, or one asked as another account, is dropped (CTL2-8).
    fn store_maps(&self, turn: u64, identity: Option<String>, mine: &MapsMine) -> bool {
        let mut stored = self.maps_mine.lock().unwrap();
        if self.maps_turn.load(Ordering::Acquire) != turn || self.maps_identity() != identity {
            return false;
        }
        *stored = Some((identity, mine.clone()));
        true
    }

    /// Signed out: the last account's list is no longer anyone's to show.
    pub(super) fn forget_maps(&self) {
        self.maps_turn.fetch_add(1, Ordering::AcqRel);
        *self.maps_mine.lock().unwrap() = None;
        self.broadcast(|| ToClient::Maps { mine: MapsMine::default() });
    }

    /// Fetches what this member added (signed out: nothing), then tells every interface.
    pub fn refresh_maps(self: &Shared) {
        let c = self.clone();
        let turn = self.maps_turn.fetch_add(1, Ordering::AcqRel) + 1;
        std::thread::spawn(move || {
            let identity = c.maps_identity();
            let mine = match c.account_api() {
                None => MapsMine::default(),
                Some((api, token)) => match api.mine(&token) {
                    Ok(m) => MapsMine { worlds: m.worlds, error: None },
                    // What was shown stays beside the error, when it is this account's.
                    Err(e) => MapsMine { worlds: c.maps().map(|m| m.worlds).unwrap_or_default(), error: Some(e.to_string()) },
                },
            };
            if !c.store_maps(turn, identity, &mine) {
                return;
            }
            c.broadcast(|| ToClient::Maps { mine: mine.clone() });
            if let Some(ctx) = c.repaint.lock().unwrap().as_ref() {
                ctx.request_repaint();
            }
        });
    }

    fn start_upload(&self, world_id: &str, world_name: &str, kind: UploadKind, paths: Vec<PathBuf>) -> anyhow::Result<u64> {
        anyhow::ensure!(orbiters_core::vrchat_log::is_world_id(world_id), "{}", tr("Not a VRChat world."));
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
                Err(anyhow::anyhow!("{}", tr("This is already being sent.")))
            } else {
                Ok(id)
            }
        })
    }

    /// Reports the bytes sent, at most every 100 ms (each change goes to every interface).
    fn progress(self: &Shared, id: u64) -> impl FnMut(u64, u64) + Send + 'static {
        let c = self.clone();
        let mut last = super::shares::ago(std::time::Duration::from_secs(1));
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
                (UploadState::Done { .. }, UploadKind::Preview) => (NoticeKind::Success, tr!("3D preview of {name} shared on the Map explorer.", name = name)),
                (UploadState::Done { added: 0, .. }, UploadKind::Pictures) => {
                    (NoticeKind::Info, tr!("These pictures of {name} were already on the Map explorer.", name = name))
                }
                (UploadState::Done { added, skipped }, UploadKind::Pictures) => {
                    // Whole sentences: the plural and the order are each language's own.
                    let text = match (*added == 1, *skipped > 0) {
                        (true, false) => tr!("1 picture of {name} shared on the Map explorer.", name = name),
                        (false, false) => tr!("{added} pictures of {name} shared on the Map explorer.", added = added, name = name),
                        (true, true) => tr!("1 picture of {name} shared on the Map explorer ({skipped} already there).", name = name, skipped = skipped),
                        (false, true) => tr!("{added} pictures of {name} shared on the Map explorer ({skipped} already there).", added = added, name = name, skipped = skipped),
                    };
                    (NoticeKind::Success, text)
                }
                (UploadState::Failed { message }, _) => (NoticeKind::Error, tr!("Could not share to the Map explorer: {message}", message = message)),
                _ => (NoticeKind::Info, String::new()),
            };
            if !message.is_empty() {
                self.notify(tone, message);
            }
        }
        self.refresh_maps();
    }

    /// The account of the member of `epoch`, whom the window showed as it was asked: the whole upload is theirs.
    fn uploading_as(self: &Shared, epoch: u64) -> anyhow::Result<(orbiters_core::account::Api, String)> {
        let acting = self.n4n0.acting(epoch);
        let Some(acting) = acting else { anyhow::bail!("{}", tr("That was for the Orbiters account signed in before: nothing was done.")) };
        acting.account().ok_or_else(|| anyhow::anyhow!("{}", tr("Sign in to Orbiters first.")))
    }

    /// Shares pictures of a world: VRChat screenshots, sent as they are, 12 per request.
    pub fn upload_pictures(self: &Shared, world_id: String, world_name: String, paths: Vec<PathBuf>, epoch: u64) -> anyhow::Result<()> {
        let (api, token) = self.uploading_as(epoch)?;
        anyhow::ensure!(!paths.is_empty(), "{}", tr("Choose pictures to share."));
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
            anyhow::bail!("{}", tr!("{file} is over 25 MB.", file = paths[i].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        }
        let total: u64 = sizes.iter().sum();
        let (mut added, mut skipped, mut done) = (0, 0, 0u64);
        for (chunk, chunk_sizes) in paths.chunks(PICTURES_PER_REQUEST).zip(sizes.chunks(PICTURES_PER_REQUEST)) {
            let mut parts = Vec::with_capacity(chunk.len());
            let mut taken = Vec::with_capacity(chunk.len());
            // PRIV-8: each picture goes as a copy without its metadata (who took it, who else was there, the instance),
            // pixels as they are; the Map explorer knows it by that copy's hash as well (`worlds::map_hashes`). The
            // copies are removed when this chunk is sent or fails.
            let mut copies = Vec::with_capacity(chunk.len());
            for p in chunk {
                taken.push(visited::read_shot(p).map(|i| iso(i.taken)).unwrap_or_default());
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "picture.png".into());
                let copy = orbiters_core::png_meta::CleanCopy::of(p)?;
                // Read from the file while it is sent: twelve pictures never sit in memory at once.
                parts.push(Part::file("pictures", name, "image/png", &copy.path)?);
                copies.push(copy);
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
    pub fn upload_preview(self: &Shared, world_id: String, world_name: String, session: PathBuf, epoch: u64) -> anyhow::Result<()> {
        let (api, token) = self.uploading_as(epoch)?;
        anyhow::ensure!(reconstruction::read_report(&session).is_some(), "{}", tr("Reconstruct this session first."));
        let id = self.start_upload(&world_id, &world_name, UploadKind::Preview, vec![session.clone()])?;
        let c = self.clone();
        std::thread::spawn(move || {
            let result = reconstruction::result_dir(&session);
            let edits_now = || orbiters_reconstruct::edit::generation(&result);
            let state = match web_preview(&session).and_then(|prepared: Prepared| {
                // The picture only when drawn for these edits (capture_picture), and the edits still those prepared.
                let picture = crate::capture_picture::jpeg(&session);
                anyhow::ensure!(edits_now()? == prepared.generation, "{}", tr("Its splat edits changed while it was being prepared: upload it again."));
                // Every part (trees, sky, picture) of the one result they were made from, read before this (C1).
                prepared.ensure_current(&session)?;
                let Prepared { files, sky, metadata, generation, .. } = prepared;
                let total = files.len() + sky.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
                c.update_if(|s| match s.uploads.iter_mut().find(|u| u.id == id) {
                    Some(u) => {
                        u.state = UploadState::Sending;
                        u.total = total;
                        true
                    }
                    None => false,
                });
                api.add_preview(&token, &world_id, files, sky.as_deref(), picture, &metadata, c.progress(id))?;
                // A world's preview is public once uploaded: edited meanwhile, it says so (the next upload replaces it).
                anyhow::ensure!(
                    edits_now()? == generation,
                    "{}",
                    tr("Its splats were changed in its 3D view while it was uploading: the world's preview has them as they were. Upload it again.")
                );
                Ok(())
            }) {
                Ok(()) => UploadState::Done { added: 1, skipped: 0 },
                Err(e) => UploadState::Failed { message: e.to_string() },
            };
            c.finish_upload(id, state);
        });
        Ok(())
    }
}

/// What a share uploads of a session ([`web_preview`]): VRChat's three trees, the sky map, the upload's metadata, and
/// the generation of the splat edits they were made with (orbiters_reconstruct::edit::generation), which the upload
/// record keeps and the share checks again before anything is shared.
pub struct Prepared {
    pub files: PreviewFiles,
    /// A copy of the sky map taken while the result was the one prepared (crate::upload_prep::Pinned).
    pub sky: Option<crate::upload_prep::Pinned>,
    pub metadata: serde_json::Value,
    pub generation: String,
    /// The result the parts were made from (CHALLENGER-C1), read when the preparation started and checked when it ended.
    pub identity: crate::upload_prep::ResultIdentity,
}

impl Prepared {
    /// Refuses when the session's result is no longer the one these parts were made from (reconstructed again
    /// meanwhile): called once everything to send is in hand, so that all of it comes from one result or none is sent.
    pub fn ensure_current(&self, session: &Path) -> anyhow::Result<()> {
        let now = crate::upload_prep::ResultIdentity::read(session);
        anyhow::ensure!(self.identity.complete && now.complete, "{}", tr("Its last reconstruction was only partly saved (a file was open in another program): reconstruct it again to share it."));
        anyhow::ensure!(now == self.identity, "{}", tr("It was reconstructed again while it was being prepared: share it again."));
        Ok(())
    }
}

/// A session's splats (without those its edits hide or delete: orbiters_reconstruct::edit) as VRChat's splat trees
/// (their merged boxes tuned on the GPU when there is one; Orbiters makes the website's `.splat` from the PC one's
/// leaves), its sky map (sky.png as trained, when training learnt one) and the upload's metadata: the start view
/// (right-handed like the files: Unity's Z negated) at the eye height of the person who captured, over the ground they
/// stood on (its height and that eye height too: crate::views::eye_level), the held-out quality and when the session
/// was recorded.
pub(super) fn web_preview(session: &Path) -> anyhow::Result<Prepared> {
    // Prepared in the background since its reconstruction finished, or being prepared: that cache answers.
    crate::upload_prep::claim(session);
    prepare(session, orbiters_reconstruct::gpu::DeviceOptions::default())
}

/// Tries of a preparation whose splat edits changed while it ran.
const PREPARE_TRIES: usize = 3;

/// [`web_preview`]'s work on a GPU opened with `device` (the background preparation's: low priority, maybe the
/// integrated GPU: crate::upload_prep::run). The edits are another process's (the window saves them as they are made):
/// their generation is read before and after, and the trees (made now, or from the cache, whose key holds the
/// generation too) answer only when they are of that very generation, else it prepares again; edits that cannot be read
/// stop it.
pub fn prepare(session: &Path, device: orbiters_reconstruct::gpu::DeviceOptions) -> anyhow::Result<Prepared> {
    let result = reconstruction::result_dir(session);
    // A result left half replaced (some of its files were held) is no one result: nothing is prepared from it. One whose
    // replacement finished but was never unmarked (a crash) is unmarked first, when its parts verify (night4).
    let _ = reconstruction::settle_replacing(&result);
    anyhow::ensure!(crate::upload_prep::ResultIdentity::read(session).complete, "{}", tr("Its last reconstruction was only partly saved (a file was open in another program): reconstruct it again to share it."));
    for _ in 0..PREPARE_TRIES {
        let before = orbiters_reconstruct::edit::generation(&result)?;
        let prepared = prepare_once(session, device)?;
        let after = orbiters_reconstruct::edit::generation(&result)?;
        if prepared.generation == before && after == before && crate::upload_prep::ResultIdentity::read(session) == prepared.identity {
            return Ok(prepared);
        }
        eprintln!("the splat edits or the result of {} changed while it was prepared: again", session.display());
    }
    anyhow::bail!("{}", tr("Its splat edits kept changing while it was being prepared: share it again once they are done."))
}

/// One [`prepare`]: the trees and the generation they were made with.
fn prepare_once(session: &Path, device: orbiters_reconstruct::gpu::DeviceOptions) -> anyhow::Result<Prepared> {
    use orbiters_reconstruct::timing::PrepStage;
    let _total = PrepStage::new("total");
    let _preparing = crate::upload_prep::PREPARING.lock().unwrap_or_else(|e| e.into_inner());
    let identity = crate::upload_prep::ResultIdentity::read(session);
    let result = reconstruction::result_dir(session);
    let key = crate::upload_prep::tree_key();
    let stage = PrepStage::new("cache fingerprint"); // i18n: not shown
    let fingerprint = crate::upload_prep::Fingerprint::read(session, &result).ok();
    drop(stage);
    let stage = PrepStage::new("cache read"); // i18n: not shown
    let cached = fingerprint.as_ref().and_then(|fingerprint| Some((crate::upload_prep::load(&result, fingerprint, key)?, fingerprint.generation()?.to_string())));
    drop(stage);
    if let Some(((files, metadata), generation)) = cached {
        let stage = PrepStage::new("sky");
        let sky = crate::upload_prep::sky(&result).map(|p| crate::upload_prep::Pinned::copy(&p)).transpose()?;
        drop(stage);
        return Ok(Prepared { files, sky, metadata, generation, identity });
    }
    let stage = PrepStage::new("read result"); // i18n: not shown
    // Without what the 3D view hid or deleted (orbiters_reconstruct::edit): an edit that cannot be applied stops the
    // share with why, rather than letting a removed avatar back in.
    let (g, _, generation) = orbiters_reconstruct::edit::read_edited(&result.join(reconstruction::PLY_FILE))?;
    drop(stage);
    let stage = PrepStage::new("start view"); // i18n: not shown
    let (views, start) = crate::views::recorded(session)?;
    let (v, ground) = crate::views::eye_level(session, &g, &views[start]);
    let depth = crate::views::depth_ahead(&g, &v);
    let target = [v.position[0] + v.forward[0] * depth, v.position[1] + v.forward[1] * depth, v.position[2] + v.forward[2] * depth];
    drop(stage);
    let stage = PrepStage::new("session cameras"); // i18n: not shown
    let cameras = orbiters_reconstruct::tree_tune::session_cameras(session)?;
    drop(stage);
    let stage = PrepStage::new("gpu setup"); // i18n: not shown
    let mut gpu = orbiters_reconstruct::gpu::Gpu::with_options(orbiters_reconstruct::gpu::DeviceOptions {
        profile: std::env::var_os("ORBITERS_PREP_PROFILE").is_some(),
        ..device
    })
    .ok();
    if std::env::var_os("ORBITERS_PREP_TIMING").is_some() {
        eprintln!("upload preparation adapter: {}", gpu.as_ref().map_or("CPU (untuned)", |g| g.adapter.as_str()));
    }
    drop(stage);
    // Names this upload in its three trees, so a world can tell when a slot's files have been replaced.
    // What a preparation stopped by a capture finished goes on (crate::upload_prep::PartialFile): only for tuned trees of
    // this very result, whose splat edits its fingerprint holds.
    let mut kept = fingerprint.as_ref().filter(|f| gpu.is_some() && f.generation() == Some(generation.as_str())).map(|f| crate::upload_prep::PartialFile::new(&result, f));
    let options = orbiters_reconstruct::tree_tune::ExportOptions::default();
    let trees = orbiters_reconstruct::tree_tune::vrchat_files_kept(&g, &cameras, gpu.as_mut(), key, &options, kept.as_mut().map(|k| k as &mut dyn orbiters_reconstruct::tree_tune::Keep), |_| {})?;
    if let Some(gpu) = &gpu {
        if let Some(p) = &gpu.profiler {
            eprintln!("upload preparation GPU span: {:?}; passes: {:?}", p.span.get(), p.totals.borrow());
        }
    }
    let files = PreviewFiles { vrchat_pc: trees.pc, vrchat_quest: trees.quest, vrchat_coarse: trees.coarse };
    let flip = |p: [f32; 3]| [p[0], p[1], -p[2]];
    let report = reconstruction::read_report(session);
    let recorded = bundle::read_manifest(session).map(|m| m.created_utc).ok();
    let metadata = serde_json::json!({
        "view": { "position": flip(v.position), "target": flip(target), "floor": ground.floor, "eyeHeight": ground.eye_height },
        "quality": report.and_then(|r| r.psnr_held_out),
        "capturedAt": recorded,
    });
    let stage = PrepStage::new("sky");
    let sky = crate::upload_prep::sky(&result).map(|p| crate::upload_prep::Pinned::copy(&p)).transpose()?;
    drop(stage);
    // Never persist the untuned CPU fallback, or outputs whose sources changed while preparation was running.
    // A full/read-only disk must not prevent the upload; the next share can prepare again.
    // Under the key of the generation the trees were made with only (not one the edits passed through meanwhile).
    if let Some(fingerprint) = fingerprint.as_ref().filter(|f| gpu.is_some() && f.generation() == Some(generation.as_str())) {
        let stage = PrepStage::new("cache write"); // i18n: not shown
        if crate::upload_prep::Fingerprint::read(session, &result).ok().as_ref() == Some(fingerprint) {
            if let Err(e) = crate::upload_prep::save(&result, fingerprint, &files, &metadata) {
                eprintln!("Could not cache upload preparation: {e:#}");
            }
        }
        drop(stage);
    }
    Ok(Prepared { files, sky, metadata, generation, identity })
}


#[cfg(test)]
mod tests {
    use super::*;

    /// CTL2-8: of two fetches, the older answer arriving last is dropped; one asked as another account is dropped; a
    /// sign-out forgets the list.
    #[test]
    fn only_the_newest_answer_of_the_account_signed_in_is_kept() {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        let list = |n: usize| MapsMine { worlds: vec![Default::default(); n], error: None };
        let first = ctl.maps_turn.fetch_add(1, Ordering::AcqRel) + 1;
        let second = ctl.maps_turn.fetch_add(1, Ordering::AcqRel) + 1;
        assert!(ctl.store_maps(second, None, &list(2)));
        assert!(!ctl.store_maps(first, None, &list(1)), "the older answer, arriving last");
        assert_eq!(ctl.maps().map(|m| m.worlds.len()), Some(2));
        assert!(!ctl.store_maps(second, Some("another account".into()), &list(5)), "asked as another account");
        assert_eq!(ctl.maps().map(|m| m.worlds.len()), Some(2));
        ctl.forget_maps();
        assert!(ctl.maps().is_none());
        assert!(!ctl.store_maps(second, None, &list(3)), "a fetch from before the sign-out");
    }

    /// Local preparation only; never contacts a server. Run in release mode, one GPU test at a time.
    #[test]
    #[ignore]
    fn upload_prep_timings() {
        let sessions = std::env::var("ORBITERS_TEST_SESSION").expect("ORBITERS_TEST_SESSION (semicolon-separated folders)");
        for (index, session) in sessions.split(';').filter(|s| !s.is_empty()).enumerate() {
            eprintln!("upload preparation session {index}: {session}");
            let Prepared { files, sky, metadata, .. } = web_preview(Path::new(session)).expect("prepare");
            eprintln!("upload preparation bytes: {}; sky: {}; metadata: {metadata}", files.len(), sky.is_some());
            if let Some(out) = std::env::var_os("ORBITERS_PREP_OUT") {
                let out = PathBuf::from(out).join(index.to_string());
                std::fs::create_dir_all(&out).unwrap();
                for (name, bytes) in
                    [("pc.tree", &files.vrchat_pc), ("quest.tree", &files.vrchat_quest), ("coarse.tree", &files.vrchat_coarse)]
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
        let Prepared { files, sky, metadata, .. } = web_preview(Path::new(&env("ORBITERS_TEST_SESSION"))).unwrap();
        println!("{:.1} MB web preview and VRChat trees in {:?}: {metadata}", files.len() as f64 / 1e6, t.elapsed());
        assert_eq!(&files.vrchat_pc[..8], orbiters_reconstruct::tree::MAGIC);
        let view = &metadata["view"];
        assert_eq!(view["position"].as_array().unwrap().len(), 3);
        let before = api.world(&world).unwrap().unwrap().previews;
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let s = sent.clone();
        let total = files.len();
        api.add_preview(&env("ORBITERS_TEST_TOKEN"), &world, files, sky.as_deref(), None, &metadata, move |n, _| s.store(n, Ordering::Relaxed)).unwrap();
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
            let Prepared { files, sky, metadata, .. } = web_preview(Path::new(session)).unwrap();
            println!("{world}: {:.1} MB in {:.1} s, sky {}", files.len() as f64 / 1e6, t.elapsed().as_secs_f32(), sky.is_some());
            api.add_preview(&account.token, world, files, sky.as_deref(), None, &metadata, |_, _| {}).unwrap();
            println!("{world}: shared");
        }
    }
}
