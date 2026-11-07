//! The FPS booster's one state, and its words, shared by the window and the overlay so that they can never disagree: each draws
//! from [`Status`] in the service's snapshot, and the level is a setting like any other. What decides is
//! `orbiters_booster::engine`, in the overlay; what acts on other programs is `controller/booster.rs`, in the service.

use crate::state::Snapshot;
use orbiters_booster::classify::Limit;
use orbiters_booster::engine::{Line, Report, Request};
use orbiters_booster::guided::{Item, Known, Measured, ToPutBack};
use orbiters_booster::trial::{Comparison, Kind, Verdict};
use orbiters_booster::Level;
use orbiters_i18n::{tr, tr_ctx};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What the overlay tells the service each second.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OverlayReport {
    pub report: Report,
    /// A streamer's driver: its own frame synthesis happens where this cannot see it.
    pub streamer: bool,
    /// SteamVR's sharpness, while the booster has it changed: what it was (`None`: Auto), and what it is.
    pub sharpness: Option<(Option<f32>, f32)>,
    /// The last request done, by its number.
    pub answered: u64,
    /// A 3D panel shows: only then is there anything to test of it.
    pub panel_3d: bool,
}

/// All both surfaces show.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub overlay: OverlayReport,
    /// Reports are arriving: the overlay runs and watches.
    pub watching: bool,
    /// The programs lowered now, by name, with how many of each.
    pub lowered: Vec<(String, u32)>,
    /// The record of what was changed could not be read: nothing is changed, and it says so.
    pub journal_unreadable: bool,
    /// What a surface asked of the overlay, numbered: a request stays in the snapshot until it is answered.
    pub request: Option<(u64, Request)>,
    /// What the person changed in VRChat's menu and should have back, after leaving step 3.
    pub to_put_back: Vec<ToPutBack>,
    /// What the active account has set, read only: `None` says to look in VRChat's menu.
    pub max_shown: Option<Known>,
    pub hide_beyond: Option<Known>,
    pub cpu_total: f32,
    pub listed_cores: f32,
    pub top: Vec<(String, f32)>,
    /// Test builds: what only testing needs.
    pub test_tools: bool,
    pub test_load: bool,
    pub test_report: Option<PathBuf>,
}

/// What a surface asks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Command {
    /// For the overlay, where the frames are.
    Overlay(Request),
    /// "Put everything back": the level goes to Off, and with it all the booster changed.
    PutBack,
    OpenLogs,
    /// Done in VRChat's menu on step 3's list: the old value goes on the list of what to put back.
    GuidedDone(Item),
    /// The person put those back.
    PutBackDone,
    /// Test builds: a small program of its own on the allowlist, for step 1 to have something to act on.
    TestLoad(bool),
    /// Test builds: writes what was measured to the logs folder.
    WriteTestReport,
}

/// Which step a slider's stop is, in the settings' words.
pub fn level_name(level: Level) -> &'static str {
    match level {
        Level::Off => tr_ctx("FPS booster level", "Off"),
        Level::Background => tr_ctx("FPS booster level", "Background"),
        Level::Picture => tr_ctx("FPS booster level", "Picture"),
        Level::Avatars => tr_ctx("FPS booster level", "Avatars"),
    }
}

/// The one line each stop has.
pub fn level_line(level: Level) -> &'static str {
    match level {
        Level::Off => tr("Nothing changes. Orbiters only watches your frames."),
        Level::Background => tr("Keeps other programs out of VRChat's way. VR looks the same."),
        Level::Picture => tr("Also lowers sharpness, but only if that really adds frames. 3D pictures finish after you leave VRChat."),
        Level::Avatars => tr("Also helps you limit other people's avatars. Some people will look simpler, turn into a stand-in, or disappear for you."),
    }
}

/// What moving to Avatars asks, once.
pub fn avatars_question() -> &'static str {
    tr("This changes how other people look to you. They won't be told, and you can still press Show Avatar on anyone.")
}

fn limit_line(limit: Limit) -> &'static str {
    match limit {
        Limit::Processor => tr("Your limit: the processor"),
        Limit::GraphicsCard => tr("Your limit: the graphics card"),
        Limit::Both => tr("Your limit: both"),
        Limit::Fine => tr("Your limit: nothing right now"),
        Limit::NotSure => tr("Not sure yet"),
    }
}

/// The one status line that matters now.
pub fn status_line(snap: &Snapshot) -> &'static str {
    let booster = &snap.booster;
    if booster.journal_unreadable {
        return tr("The booster's record of your settings can't be read, so it changes nothing.");
    }
    if !snap.steamvr.running {
        return tr("Works in VR.");
    }
    if !snap.settings.steamvr_overlay {
        return tr("Needs Orbiters' SteamVR overlay.");
    }
    if !booster.watching {
        return tr("Starts when VRChat has the scene.");
    }
    line_words(booster.overlay.report.line, booster.overlay.report.limit)
}

/// A line of the report in words.
pub fn line_words(line: Line, limit: Limit) -> &'static str {
    match line {
        Line::Limit | Line::Watching => limit_line(limit),
        Line::Testing => tr("Testing for about a minute"),
        Line::Kept => tr("Kept: more real frames"),
        Line::GainNotKept => tr("More real frames when tested, put back: this step doesn't keep it"),
        Line::NoGain => tr("No gain measured, put back"),
        Line::TooMuchChanging => tr("Too much was changing to tell."),
        Line::NotTaken => tr("VRChat didn't take this change while running."),
        Line::ProcessorStays => tr("Your limit is the processor. Lowering sharpness wouldn't add frames, so it stays."),
        Line::NothingCompeting => tr("Nothing's competing with VRChat right now."),
        Line::HandChanged => tr("You changed SteamVR's resolution while the booster was on, so it stays."),
        Line::AnotherApp => tr("Another app is controlling resolution, so the booster leaves it alone."),
        Line::Refused => tr("SteamVR wouldn't take this change, so nothing changed."),
        Line::NoScene => tr("Starts when VRChat has the scene."),
        Line::CannotTestNow => tr("That can't be tested right now: VRChat needs the scene, with the dashboard closed."),
    }
}

/// What a comparison compared.
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Programs => tr("Other programs in efficiency mode"),
        Kind::Sharpness => tr("Lower sharpness"),
        Kind::PanelQuality => tr("Lighter 3D panels"),
        Kind::ReconstructionHold => tr("3D pictures waiting"),
    }
}

/// The figures of a comparison that ran to its end, as text: the only place a gain can come from.
pub struct BeforeAfter {
    pub title: &'static str,
    pub figures: String,
    pub said: &'static str,
    pub filled: String,
}

fn whole(fps: f32) -> i32 {
    fps.round() as i32
}

fn percent(share: f32) -> i32 {
    (share * 100.0).round() as i32
}

/// Before and after, with the difference in real frames: a gain only if the comparison says so. A comparison that found
/// nothing still shows its figures, and says that it found nothing.
pub fn before_after(c: &Comparison) -> BeforeAfter {
    let (before, after) = (whole(c.before_fps), whole(c.after_fps));
    let difference = after - before;
    let figures = match c.verdict {
        Verdict::Kept => {
            orbiters_i18n::tr!("Before {before} · After {after} ({difference})", before = before, after = after, difference = format!("{difference:+}"))
        }
        _ => orbiters_i18n::tr!("Before {before} · After {after}", before = before, after = after),
    };
    let said = match c.verdict {
        Verdict::Kept if c.kept => tr("Same world, tested twice"),
        Verdict::Kept => tr("Same world, tested twice. Put back: this step doesn't keep it."),
        Verdict::NoGain => tr("No gain measured, put back"),
        Verdict::TooMuchChanging => tr("Too much was changing to tell."),
        Verdict::NotTaken => tr("VRChat didn't take this change while running."),
    };
    let filled = orbiters_i18n::tr!("Filled in by SteamVR: {before} % → {after} %", before = percent(c.before_filled), after = percent(c.after_filled));
    BeforeAfter { title: tr("Real frames a second"), figures, said, filled }
}

/// With no comparison: only what is true now, and never a gain.
pub fn right_now(snap: &Snapshot) -> Option<String> {
    let now = snap.booster.overlay.report.now?;
    Some(orbiters_i18n::tr!("Now: {fps} real frames a second at {hz} Hz. No change to compare yet.", fps = whole(now.real_fps), hz = whole(now.refresh_hz)))
}

/// Said beneath any figures, when a streamer is the driver.
pub fn streamer_line() -> &'static str {
    tr("Virtual Desktop may add its own frames. Those aren't counted.")
}

/// The list's item, in VRChat's words, in the order the person does them.
pub fn item_name(item: Item) -> &'static str {
    match item {
        Item::MaxShown => tr("In VRChat's settings, set Maximum Shown Avatars to 15."),
        Item::BlockVeryPoor => tr("In VRChat's settings, under Avatar Performance, block Very Poor avatars."),
        Item::HideBeyond => tr("In VRChat's settings, set Hide Avatars Beyond to 10 m."),
        Item::Msaa2x => tr("In VRChat's settings, set MSAA to 2x."),
        Item::MsaaOff => tr("In VRChat's settings, turn MSAA off."),
    }
}

/// What the item's limit is set to now, as the registry says, read only.
pub fn item_now(item: Item, snap: &Snapshot) -> String {
    let known = match item {
        Item::MaxShown => snap.booster.max_shown,
        Item::HideBeyond => snap.booster.hide_beyond,
        Item::BlockVeryPoor | Item::Msaa2x | Item::MsaaOff => None,
    };
    match known {
        Some(known) => orbiters_i18n::tr!("Set now: {value}", value = known_words(known)),
        None => tr("Check VRChat's menu").to_string(),
    }
}

fn known_words(known: Known) -> String {
    match known {
        Known::Off => tr_ctx("FPS booster level", "Off").to_string(),
        Known::Count(count) => count.to_string(),
        Known::Meters(meters) => orbiters_i18n::tr!("{meters} m", meters = format!("{meters:.1}")),
    }
}

/// A hand comparison's figures, labelled as what they are.
pub fn measured_words(m: &Measured) -> String {
    orbiters_i18n::tr!(
        "{before} → {after} real frames a second. You changed this; people may have moved.",
        before = whole(m.before_fps),
        after = whole(m.after_fps)
    )
}

/// An item of the list by the name of its setting in VRChat.
pub fn item_setting(item: Item) -> &'static str {
    match item {
        Item::MaxShown => tr("Maximum Shown Avatars"),
        Item::BlockVeryPoor => tr("Avatar Performance"),
        Item::HideBeyond => tr("Hide Avatars Beyond"),
        Item::Msaa2x | Item::MsaaOff => "MSAA",
    }
}

/// What to change back by hand, with what it was.
pub fn put_back_words(item: &ToPutBack) -> String {
    let name = item_setting(item.item);
    match item.was {
        Some(was) => orbiters_i18n::tr!("{name}: it was {value}", name = name, value = known_words(was)),
        None => orbiters_i18n::tr!("{name}: check VRChat's menu", name = name),
    }
}

/// What the booster has changed right now, each with its old value where there is one: nothing that is not so.
pub fn changed_now(snap: &Snapshot) -> Vec<String> {
    let booster = &snap.booster;
    let wants = booster.overlay.report.wants;
    let mut out = Vec::new();
    if let Some((was, now)) = booster.overlay.sharpness {
        out.push(match was {
            Some(was) => orbiters_i18n::tr!("SteamVR's resolution: {now}, it was {was}", now = format!("{now:.2}"), was = format!("{was:.2}")),
            None => orbiters_i18n::tr!("SteamVR's resolution: {now}, it was Auto", now = format!("{now:.2}")),
        });
    }
    for (name, count) in &booster.lowered {
        out.push(orbiters_i18n::tr!("{name} in efficiency mode ({count}), it was off", name = name, count = count));
    }
    if wants.pace_panels {
        out.push(tr("Orbiters' panels: 30 pictures a second, it was 45").to_string());
    }
    if wants.stop_preparing {
        out.push(tr("Preparing shares: stopped while VRChat has the scene").to_string());
    }
    if wants.panels_light {
        out.push(tr("3D panels: Light, unless you choose on the panel").to_string());
    }
    if snap.settings.booster.level >= Level::Picture && booster.watching && !wants.release_hold {
        out.push(tr("3D pictures: waiting until you leave VRChat").to_string());
    }
    out
}

fn one(value: f32) -> String {
    format!("{value:.1}")
}

fn none(value: f32) -> String {
    format!("{value:.0}")
}

fn share(value: f32) -> String {
    format!("{:.0}", value * 100.0)
}

/// The test report: what was measured, in the person's language. Figures only from what was measured; nothing else.
pub fn test_report(snap: &Snapshot, compositor: Option<&orbiters_booster::log::CompositorStats>) -> String {
    let report = &snap.booster.overlay.report;
    let mut lines = vec![tr("FPS booster test report").to_string(), String::new()];
    lines.push(line_words(Line::Limit, report.limit).to_string());
    lines.push(
        if report.flags_seen { tr("SteamVR's reasons for late frames were seen.") } else { tr("SteamVR never gave a reason for a late frame.") }.to_string(),
    );
    if snap.booster.overlay.streamer {
        lines.push(streamer_line().to_string());
    }
    if let Some(now) = report.now {
        lines.push(orbiters_i18n::tr!(
            "Now: {fps} real frames a second at {hz} Hz, {filled} % filled in by SteamVR, {cpu_late} % late for the processor, {gpu_late} % late for the graphics card, processor {cpu} ms (estimate), graphics card {gpu} ms.",
            fps = one(now.real_fps),
            hz = none(now.refresh_hz),
            filled = share(now.filled_share),
            cpu_late = share(now.late_cpu_share),
            gpu_late = share(now.late_gpu_share),
            cpu = one(now.app_cpu_ms),
            gpu = one(now.app_gpu_ms)
        ));
    }
    lines.push(String::new());
    lines.push(tr("Comparisons, without then with the change, in the same scene, twice:").to_string());
    for c in &report.comparisons {
        lines.push(orbiters_i18n::tr!(
            "{change}: {result} Real frames {fps_before} / {fps_after}. Filled in {filled_before} % / {filled_after} %. Late for the processor {cpu_before} % / {cpu_after} %. Late for the graphics card {gpu_before} % / {gpu_after} %. Over twice the budget {over_before} % / {over_after} %. Stalls at the switches: {stalls}.",
            change = kind_name(c.kind),
            result = before_after(c).said,
            fps_before = one(c.before_fps),
            fps_after = one(c.after_fps),
            filled_before = share(c.before_filled),
            filled_after = share(c.after_filled),
            cpu_before = share(c.before_late_cpu),
            cpu_after = share(c.after_late_cpu),
            gpu_before = share(c.before_late_gpu),
            gpu_after = share(c.after_late_gpu),
            over_before = one(c.before_over_2x * 100.0),
            over_after = one(c.after_over_2x * 100.0),
            stalls = c.stalls_at_switch
        ));
    }
    lines.push(String::new());
    lines.push(tr("By hand on the list, 30 s before and after (people may have moved):").to_string());
    let people = |p: Option<u32>| p.map_or_else(|| "?".to_string(), |p| p.to_string());
    for m in &report.guide.results {
        lines.push(orbiters_i18n::tr!(
            "{setting}: real frames {fps_before} / {fps_after}, filled in {filled_before} % / {filled_after} %, people {people_before} / {people_after}.",
            setting = item_setting(m.item),
            fps_before = one(m.before_fps),
            fps_after = one(m.after_fps),
            filled_before = share(m.before_filled),
            filled_after = share(m.after_filled),
            people_before = people(m.before_people),
            people_after = people(m.after_people)
        ));
    }
    if let Some(c) = compositor {
        lines.push(String::new());
        lines.push(orbiters_i18n::tr!(
            "SteamVR's own totals for the last session that ended: {presents} presents, {reprojected} reprojected, {dropped} dropped, application processor time {cpu} ms.",
            presents = c.presents,
            reprojected = c.reprojected,
            dropped = c.dropped,
            cpu = c.app_cpu_ms.map_or_else(|| "?".to_string(), one)
        ));
    }
    lines.join("\n") + "\n"
}

/// App processor and graphics card time, in milliseconds, marked as estimates.
pub fn times_words(snap: &Snapshot) -> Option<String> {
    let now = snap.booster.overlay.report.now?;
    Some(orbiters_i18n::tr!(
        "Processor {cpu} ms, graphics card {gpu} ms (estimate)",
        cpu = format!("{:.1}", now.app_cpu_ms),
        gpu = format!("{:.1}", now.app_gpu_ms)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_booster::guided::Item;
    use orbiters_core::settings::Settings;

    fn comparison(verdict: Verdict) -> Comparison {
        Comparison {
            kind: Kind::Programs,
            verdict,
            before_fps: 36.2,
            after_fps: 41.1,
            before_filled: 0.49,
            after_filled: 0.38,
            before_over_2x: 0.0,
            after_over_2x: 0.0,
            before_late_cpu: 0.0,
            after_late_cpu: 0.0,
            before_late_gpu: 0.0,
            after_late_gpu: 0.0,
            stalls_at_switch: 0,
            kept: verdict == Verdict::Kept,
        }
    }

    /// A difference with a plus only for a comparison that kept the change. The honest words otherwise.
    #[test]
    fn no_gain_is_never_called_a_gain() {
        let kept = before_after(&comparison(Verdict::Kept));
        assert_eq!(kept.figures, "Before 36 · After 41 (+5)");
        assert_eq!(kept.said, "Same world, tested twice");
        let told = before_after(&Comparison { kept: false, ..comparison(Verdict::Kept) });
        assert!(told.said.contains("Put back"), "M2: a gain that wasn't kept is not called kept");
        let none = before_after(&comparison(Verdict::NoGain));
        assert_eq!(none.figures, "Before 36 · After 41");
        assert_eq!(none.said, "No gain measured, put back");
        assert!(!none.figures.contains('+'));
        assert_eq!(kept.filled, "Filled in by SteamVR: 49 % → 38 %");
    }

    #[test]
    fn nothing_says_a_gain_without_a_comparison() {
        let mut snap = Snapshot::new(Settings::default());
        assert_eq!(right_now(&snap), None, "no frames, no number");
        snap.booster.overlay.report.now = Some(orbiters_booster::engine::Now { real_fps: 36.0, refresh_hz: 72.0, ..Default::default() });
        assert_eq!(right_now(&snap).unwrap(), "Now: 36 real frames a second at 72 Hz. No change to compare yet.");
    }

    #[test]
    fn it_says_why_it_waits() {
        let mut snap = Snapshot::new(Settings::default());
        snap.steamvr.running = false;
        assert_eq!(status_line(&snap), "Works in VR.");
        snap.steamvr.running = true;
        snap.settings.steamvr_overlay = false;
        assert_eq!(status_line(&snap), "Needs Orbiters' SteamVR overlay.");
        snap.settings.steamvr_overlay = true;
        assert_eq!(status_line(&snap), "Starts when VRChat has the scene.");
        snap.booster.watching = true;
        snap.booster.overlay.report.line = Line::Limit;
        snap.booster.overlay.report.limit = Limit::Processor;
        assert_eq!(status_line(&snap), "Your limit: the processor");
        snap.booster.journal_unreadable = true;
        assert!(status_line(&snap).contains("can't be read"));
    }

    /// The words that have to be said, in the words the design asks for.
    #[test]
    fn the_avatars_step_says_what_it_does_to_other_people() {
        let line = level_line(Level::Avatars);
        assert!(line.contains("look simpler") && line.contains("stand-in") && line.contains("disappear"));
        assert!(avatars_question().contains("won't be told"));
        assert!(item_name(Item::MaxShown).contains("Maximum Shown Avatars"));
    }

    #[test]
    fn the_avatar_list_says_to_check_the_menu_when_it_cannot_read() {
        let mut snap = Snapshot::new(Settings::default());
        assert_eq!(item_now(Item::MaxShown, &snap), "Check VRChat's menu");
        snap.booster.max_shown = Some(Known::Count(30));
        assert_eq!(item_now(Item::MaxShown, &snap), "Set now: 30");
        assert_eq!(item_now(Item::Msaa2x, &snap), "Check VRChat's menu");
        assert_eq!(put_back_words(&ToPutBack { item: Item::MaxShown, was: Some(Known::Count(30)) }), "Maximum Shown Avatars: it was 30");
        assert_eq!(put_back_words(&ToPutBack { item: Item::HideBeyond, was: None }), "Hide Avatars Beyond: check VRChat's menu");
    }

    #[test]
    fn what_is_changed_lists_only_what_is_so() {
        let mut snap = Snapshot::new(Settings::default());
        assert!(changed_now(&snap).is_empty(), "at Off, nothing");
        snap.booster.overlay.sharpness = Some((None, 0.85));
        snap.booster.lowered = vec![("chrome".to_string(), 12)];
        let said = changed_now(&snap);
        assert_eq!(said[0], "SteamVR's resolution: 0.85, it was Auto");
        assert_eq!(said[1], "chrome in efficiency mode (12), it was off");
    }

    /// The test report Enzo reads after the test is in the app's language, figures and all.
    #[test]
    fn the_test_report_is_translated() {
        let mut snap = Snapshot::new(Settings::default());
        snap.booster.overlay.report.comparisons = vec![comparison(Verdict::Kept)];
        snap.booster.overlay.report.now = Some(orbiters_booster::engine::Now { real_fps: 36.0, refresh_hz: 72.0, ..Default::default() });
        let english = test_report(&snap, None);
        assert!(english.starts_with("FPS booster test report\n"));
        assert!(english.contains("Real frames 36.2 / 41.1."), "{english}");
        orbiters_i18n::in_language(orbiters_i18n::Language::Fr, || {
            let french = test_report(&snap, None);
            assert!(french.starts_with("Rapport de test du booster de FPS"), "{french}");
            assert!(french.contains("36.2"), "the figures are the same: {french}");
        });
    }

    /// Nothing of this says official, approved, safe or optimized, in any stop or line: words the design forbids.
    #[test]
    fn the_forbidden_words_are_nowhere() {
        let mut said: Vec<String> = Level::ALL.iter().map(|l| level_line(*l).to_string()).collect();
        for line in [
            Line::Limit,
            Line::Testing,
            Line::Kept,
            Line::GainNotKept,
            Line::NoGain,
            Line::TooMuchChanging,
            Line::NotTaken,
            Line::ProcessorStays,
            Line::NothingCompeting,
            Line::HandChanged,
            Line::AnotherApp,
            Line::NoScene,
            Line::CannotTestNow,
        ] {
            said.push(line_words(line, Limit::Both).to_string());
        }
        let text = said.join(" ").to_lowercase();
        for forbidden in ["official", "approved", "eac-safe", "ban", "optimized", "boosted"] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
    }
}
