//! What decides: from the frames and what is going on, what limits this PC, what to test, and what should be in force. It
//! runs in the overlay, where the frames are, and says what it wants as a plain list ([`Wants`]) each turn. Whoever acts on
//! that (the overlay for SteamVR's settings, the service for other programs) compares it with what is so. A declared state
//! rather than a list of orders means there is nothing to forget: when the scene is lost, the wants are simply all off.
//!
//! It holds no clock and touches nothing, so that every line of this can be tested with made-up frames.

use crate::classify::{Classifier, Limit};
use crate::frames::{summarize, Frame, Summary};
use crate::guided::{self, Guide, Measured};
use crate::trial::{people_changed_too_much, Comparison, Kind, Side, Trial, Verdict};
use crate::Level;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Just after joining, a world is still loading things: its frames do not count.
pub const JOIN_WAIT_S: f64 = 30.0;
/// Between two tests of sharpness, or of lowering programs again after one that gained nothing.
pub const RETEST_S: f64 = 600.0;
/// After a comparison abandoned because too much changed.
pub const RETRY_S: f64 = 60.0;
/// Each sharpness step.
pub const SHARPNESS_STEP: f32 = 0.85;
/// How long a result is the one line that matters.
const RESULT_LINE_S: f64 = 30.0;
/// A hand comparison's two sides.
const HAND_S: f64 = 30.0;
/// Allowlisted programs must use this many cores together for lowering them to be worth a test.
pub const MIN_LISTED_CORES: f32 = 1.0;
/// One block of the log.
pub const BLOCK_S: f64 = 5.0;

/// What is going on, each turn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    pub now: f64,
    /// VRChat has the scene and is not loading.
    pub scene: bool,
    pub dashboard: bool,
    /// Which instance: changes when the person goes elsewhere. No world's name or ID comes here.
    pub world: u64,
    pub people: Option<u32>,
    pub recording: bool,
    pub level: Level,
    pub refresh_hz: f32,
    /// What the allowlisted programs use together now.
    pub listed_cores: f32,
    /// A 3D panel shows.
    pub panel_3d: bool,
    /// A reconstruction is running.
    pub reconstruction: bool,
}

/// What a surface asks. It comes through the service to where the frames are, numbered, so that two surfaces asking is two
/// requests and a repeated snapshot is none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    /// Test this now, whatever the limit says.
    TestAnyway(Kind),
    /// "Done" on step 3's list.
    GuidedDone,
    GuidedNext,
    GuidedBack,
    /// Test builds: holds sharpness × 0.85 whatever the limit says, to try a change by hand or a crash on.
    HoldTestChange(bool),
    /// Test builds: the overlay ends as a crash would. Done by whoever hosts this, not here.
    SimulateCrash,
}

/// What should be in force.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wants {
    /// Other programs lowered.
    pub lower_programs: bool,
    /// Orbiters' panels at 30 pictures a second instead of 45.
    pub pace_panels: bool,
    /// No preparation of uploads while VRChat has the scene.
    pub stop_preparing: bool,
    /// 3D panels at their lightest, unless the person chose on the panel.
    pub panels_light: bool,
    /// 3D pictures may go on after all: only the side of a comparison that needs it says so.
    pub release_hold: bool,
    /// SteamVR's sharpness × this (1.0: as it was).
    pub sharpness: f32,
}

impl Default for Wants {
    fn default() -> Self {
        Self { lower_programs: false, pace_panels: false, stop_preparing: false, panels_light: false, release_hold: false, sharpness: 1.0 }
    }
}

/// The one line under the slider. Text is the interface's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Line {
    /// "Your limit: …", from [`Report::limit`].
    #[default]
    Limit,
    Testing,
    Kept,
    NoGain,
    TooMuchChanging,
    NotTaken,
    /// "Your limit is the processor. Lowering sharpness wouldn't add frames, so it stays."
    ProcessorStays,
    /// "Nothing's competing with VRChat right now."
    NothingCompeting,
    HandChanged,
    AnotherApp,
    /// SteamVR would not take the change, or it could not be recorded first: nothing changed.
    Refused,
    /// "Starts when VRChat has the scene."
    NoScene,
    /// A test was asked for at a moment it cannot run.
    CannotTestNow,
    /// Only frames being read: level Off.
    Watching,
}

/// Why sharpness is left alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeftAlone {
    HandChanged,
    AnotherApp,
    Refused,
}

/// What is known right now, without comparing anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Now {
    pub real_fps: f32,
    pub refresh_hz: f32,
    pub filled_share: f32,
    pub late_cpu_share: f32,
    pub late_gpu_share: f32,
    /// Marked "estimate" wherever it shows: it may include waiting.
    pub app_cpu_ms: f32,
    pub app_gpu_ms: f32,
}

/// A comparison under way.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Testing {
    pub kind: Kind,
    pub seconds_left: f32,
    pub side: Side,
}

/// What the overlay says of itself, about once a second. Both surfaces draw from this one report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub limit: Limit,
    pub line: Line,
    pub now: Option<Now>,
    pub testing: Option<Testing>,
    /// The last completed comparison of each kind, newest first. Nothing else may ever be shown as a gain.
    pub comparisons: Vec<Comparison>,
    pub guide: Guide,
    pub wants: Wants,
    /// Whether the reason flags have fired at all: test A's question over a streamer.
    pub flags_seen: bool,
    pub left_alone: Option<LeftAlone>,
}

/// A block of the log, every five seconds of frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub summary: Summary,
    pub limit: Limit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Judged {
    Not,
    Kept,
    Refused,
}

#[derive(Clone, Copy, Debug)]
struct Hand {
    item: guided::Item,
    done_at: f64,
    before: Summary,
    before_people: Option<u32>,
}

pub struct Engine {
    classifier: Classifier,
    world: u64,
    joined_at: f64,
    last_scene: bool,
    trial: Option<Trial>,
    programs: Judged,
    programs_at: Option<f64>,
    sharpness_steps: u32,
    sharpness_at: Option<f64>,
    sharpness_tested_in: Option<u64>,
    left_alone: Option<LeftAlone>,
    comparisons: Vec<Comparison>,
    last_result: Option<(Line, f64)>,
    /// The last seconds of frames, for what is true now and for the hand comparison.
    ring: VecDeque<(f64, Frame)>,
    block: Vec<Frame>,
    block_started: Option<f64>,
    block_ready: Option<Block>,
    guide: Guide,
    hand: Option<Hand>,
    hold_test: bool,
    flags_seen: bool,
    /// Frames of the running trial, for the raw log.
    raw: Vec<Frame>,
    raw_kind: Option<Kind>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            classifier: Classifier::default(),
            world: 0,
            joined_at: 0.0,
            last_scene: false,
            trial: None,
            programs: Judged::Not,
            programs_at: None,
            sharpness_steps: 0,
            sharpness_at: None,
            sharpness_tested_in: None,
            left_alone: None,
            comparisons: Vec::new(),
            last_result: None,
            ring: VecDeque::new(),
            block: Vec::new(),
            block_started: None,
            block_ready: None,
            guide: Guide::default(),
            hand: None,
            hold_test: false,
            flags_seen: false,
            raw: Vec::new(),
            raw_kind: None,
        }
    }

    /// New frames, oldest first, while VRChat has the scene and is not loading. `countable`: no dashboard, not just joined,
    /// so they may say what limits this PC.
    pub fn push(&mut self, inputs: &Inputs, frames: &[Frame]) {
        let now = inputs.now;
        // A comparison's frames are of two different states, and say nothing of what limits this PC as it stands.
        let countable = !inputs.dashboard && now - self.joined_at >= JOIN_WAIT_S && self.trial.is_none();
        for frame in frames {
            self.flags_seen |= frame.flags & (crate::frames::LATE_CPU | crate::frames::LATE_GPU) != 0;
            self.ring.push_back((now, *frame));
            if countable {
                self.classifier.push(now, *frame);
            }
            if let Some(trial) = self.trial.as_mut() {
                trial.push(now, *frame);
                self.raw.push(*frame);
            }
            self.block.push(*frame);
        }
        while self.ring.front().is_some_and(|(at, _)| now - at > HAND_S + 1.0) {
            self.ring.pop_front();
        }
        if !countable {
            self.classifier.ignore();
        }
    }

    fn since(&self, now: f64, seconds: f64) -> Vec<Frame> {
        self.ring.iter().filter(|(at, _)| now - at <= seconds).map(|(_, f)| *f).collect()
    }

    /// What a block of the log has, once five seconds of frames are in.
    pub fn take_block(&mut self) -> Option<Block> {
        self.block_ready.take()
    }

    /// The raw frames of a comparison so far, for its log only.
    pub fn take_raw_if_any(&mut self) -> Option<(Vec<Frame>, Kind)> {
        let kind = self.raw_kind?;
        (!self.raw.is_empty() && (self.raw.len() >= 256 || self.trial.is_none())).then(|| (std::mem::take(&mut self.raw), kind))
    }

    /// Somebody else has SteamVR's resolution: stop touching it.
    pub fn leave_sharpness_alone(&mut self, why: LeftAlone) {
        self.left_alone = Some(why);
        self.sharpness_steps = 0;
        self.hold_test = false;
        if self.trial.as_ref().is_some_and(|t| t.kind == Kind::Sharpness) {
            self.trial = None;
        }
    }

    pub fn request(&mut self, inputs: &Inputs, request: Request) {
        let now = inputs.now;
        match request {
            Request::TestAnyway(kind) => {
                if self.trial.is_some() || !inputs.scene || inputs.dashboard || inputs.recording || (kind == Kind::Sharpness && self.left_alone.is_some()) {
                    self.last_result = Some((Line::CannotTestNow, now));
                    return;
                }
                let keep = match kind {
                    Kind::Programs => inputs.level >= Level::Background,
                    Kind::Sharpness => inputs.level >= Level::Picture,
                    Kind::PanelQuality | Kind::ReconstructionHold => false,
                };
                self.start(kind, inputs, keep);
            }
            Request::GuidedDone => {
                let Some(item) = self.guide.item() else { return };
                let frames = self.since(now, HAND_S);
                let Some(before) = summarize(&frames, inputs.refresh_hz) else {
                    self.last_result = Some((Line::CannotTestNow, now));
                    return;
                };
                self.hand = Some(Hand { item, done_at: now, before, before_people: inputs.people });
            }
            Request::GuidedNext => self.guide.at = (self.guide.at + 1).min(self.guide.items.len().saturating_sub(1)),
            Request::GuidedBack => self.guide.at = self.guide.at.saturating_sub(1),
            Request::HoldTestChange(on) => self.hold_test = on,
            Request::SimulateCrash => {}
        }
    }

    fn start(&mut self, kind: Kind, inputs: &Inputs, keep: bool) {
        self.raw_kind = Some(kind);
        self.trial = Some(Trial::new(kind, inputs.now, inputs.refresh_hz, inputs.people, keep));
        if kind == Kind::Sharpness {
            self.sharpness_at = Some(inputs.now);
            self.sharpness_tested_in = Some(inputs.world);
        }
    }

    fn finish_trial(&mut self, inputs: &Inputs) {
        let Some(trial) = self.trial.take() else { return };
        let Some(result) = trial.finish() else {
            self.abandon(trial.kind, inputs.now);
            return;
        };
        self.comparisons.retain(|c| c.kind != result.kind);
        self.comparisons.insert(0, result);
        let line = match result.verdict {
            Verdict::Kept => Line::Kept,
            Verdict::NoGain => Line::NoGain,
            Verdict::TooMuchChanging => Line::TooMuchChanging,
            Verdict::NotTaken => Line::NotTaken,
        };
        self.last_result = Some((line, inputs.now));
        match trial.kind {
            Kind::Programs => {
                self.programs_at = Some(inputs.now);
                self.programs = if result.verdict == Verdict::Kept && trial.keep_if_better { Judged::Kept } else { Judged::Refused };
            }
            Kind::Sharpness => {
                if result.verdict == Verdict::Kept && trial.keep_if_better {
                    self.sharpness_steps += 1;
                }
            }
            Kind::PanelQuality | Kind::ReconstructionHold => {}
        }
    }

    /// Stopped because the scene stopped being the same: nothing can be said, and it may try again soon.
    fn abandon(&mut self, kind: Kind, now: f64) {
        self.trial = None;
        self.last_result = Some((Line::TooMuchChanging, now));
        if kind == Kind::Programs {
            self.programs = Judged::Not;
            self.programs_at = Some(now - (RETEST_S - RETRY_S));
        }
    }

    /// The sharpness a step would be at, were it kept: not below the floor, which whoever writes it checks too.
    fn sharpness_of(steps: u32) -> f32 {
        SHARPNESS_STEP.powi(steps as i32)
    }

    /// One turn. `request`, if any, was already numbered and checked by the caller.
    pub fn tick(&mut self, inputs: &Inputs, request: Option<Request>) -> Wants {
        let now = inputs.now;
        if inputs.world != self.world {
            // Another world: nothing said about the last one holds, and what was changed goes back.
            self.world = inputs.world;
            self.joined_at = now;
            self.classifier.forget();
            self.programs = Judged::Not;
            self.programs_at = None;
            self.sharpness_steps = 0;
            self.hand = None;
            if let Some(trial) = self.trial.take() {
                self.abandon(trial.kind, now);
            }
        }
        if inputs.scene && !self.last_scene {
            self.classifier.ignore();
        }
        self.last_scene = inputs.scene;
        if let Some(request) = request {
            self.request(inputs, request);
        }
        let limit = self.classifier.confirmed();
        self.follow_guide(inputs, limit);
        self.follow_hand(inputs);
        self.gather_block(inputs, limit);
        if !inputs.scene {
            // The scene is lost: everything goes back, and what was concluded must be concluded again.
            self.programs = Judged::Not;
            self.sharpness_steps = 0;
            if let Some(trial) = self.trial.take() {
                self.abandon(trial.kind, now);
            }
            return Wants::default();
        }
        if !inputs.dashboard && now - self.joined_at >= JOIN_WAIT_S {
            self.classifier.look(now, inputs.refresh_hz);
        }
        // A comparison stops when what it compares stopped being the same.
        if let Some(trial) = &self.trial {
            if inputs.dashboard || inputs.recording || people_changed_too_much(trial.people_at_start, inputs.people) {
                let kind = trial.kind;
                self.abandon(kind, now);
            } else if trial.done(now) {
                self.finish_trial(inputs);
            }
        }
        if self.trial.is_none() {
            self.maybe_start(inputs, self.classifier.confirmed());
        }
        self.wants(inputs)
    }

    fn maybe_start(&mut self, inputs: &Inputs, limit: Limit) {
        let now = inputs.now;
        if inputs.dashboard || inputs.recording || now - self.joined_at < JOIN_WAIT_S {
            return;
        }
        let aged = |at: Option<f64>, wait: f64| at.is_none_or(|at| now - at >= wait);
        if inputs.level >= Level::Background
            && self.programs == Judged::Not
            && matches!(limit, Limit::Processor | Limit::Both)
            && inputs.listed_cores >= MIN_LISTED_CORES
            && aged(self.programs_at, RETEST_S)
        {
            self.start(Kind::Programs, inputs, true);
            return;
        }
        if inputs.level >= Level::Picture
            && self.left_alone.is_none()
            && matches!(limit, Limit::GraphicsCard | Limit::Both)
            && aged(self.sharpness_at, RETEST_S)
        {
            // At most one test a world, but one that was kept and still leaves the graphics card late may go a step further.
            let another_step = self.sharpness_steps > 0;
            if self.sharpness_tested_in != Some(inputs.world) || another_step {
                self.start(Kind::Sharpness, inputs, true);
            }
        }
    }

    /// What should be in force now, in VRChat's scene.
    fn wants(&self, inputs: &Inputs) -> Wants {
        let level = inputs.level;
        let limit = self.classifier.confirmed();
        let graphics = matches!(limit, Limit::GraphicsCard | Limit::Both);
        let mut wants = Wants {
            lower_programs: level >= Level::Background && self.programs == Judged::Kept,
            pace_panels: level >= Level::Background && (graphics || limit == Limit::NotSure),
            stop_preparing: level >= Level::Background,
            panels_light: level >= Level::Picture && graphics,
            release_hold: false,
            sharpness: if level >= Level::Picture { Self::sharpness_of(self.sharpness_steps) } else { 1.0 },
        };
        if self.hold_test {
            wants.sharpness = wants.sharpness.min(SHARPNESS_STEP);
        }
        if let Some(trial) = &self.trial {
            let with = trial.side(inputs.now) == Side::With;
            match trial.kind {
                Kind::Programs => wants.lower_programs = with,
                Kind::Sharpness => wants.sharpness = Self::sharpness_of(self.sharpness_steps + with as u32),
                Kind::PanelQuality => wants.panels_light = with,
                // Waiting is step 2's: without it, the job may go on.
                Kind::ReconstructionHold => wants.release_hold = !with,
            }
        }
        wants
    }

    fn follow_guide(&mut self, inputs: &Inputs, limit: Limit) {
        let items = guided::list_for(limit);
        if self.guide.items != items {
            // The list follows the limit: the person stays on the item they were on when it is still there.
            let current = self.guide.item();
            self.guide.at = current.and_then(|c| items.iter().position(|i| *i == c)).unwrap_or(0);
            self.guide.items = items;
        }
        self.guide.likely = guided::likely_to_help(limit);
        if inputs.level < Level::Avatars {
            self.hand = None;
        }
    }

    /// Thirty seconds after Done, the two sides of it. A world changed meanwhile says nothing, and `tick` forgot it.
    fn follow_hand(&mut self, inputs: &Inputs) {
        let Some(hand) = self.hand else {
            self.guide.measuring = None;
            return;
        };
        let elapsed = inputs.now - hand.done_at;
        if elapsed < HAND_S {
            self.guide.measuring = Some((hand.item, (HAND_S - elapsed) as f32));
            return;
        }
        self.hand = None;
        self.guide.measuring = None;
        let frames = self.since(inputs.now, HAND_S);
        let Some(after) = summarize(&frames, inputs.refresh_hz) else { return };
        self.guide.results.push(Measured {
            item: hand.item,
            before_fps: hand.before.real_fps,
            after_fps: after.real_fps,
            before_filled: hand.before.filled_share,
            after_filled: after.filled_share,
            before_people: hand.before_people,
            after_people: inputs.people,
        });
        if self.guide.results.len() > 12 {
            self.guide.results.remove(0);
        }
    }

    fn gather_block(&mut self, inputs: &Inputs, limit: Limit) {
        let started = *self.block_started.get_or_insert(inputs.now);
        if inputs.now - started < BLOCK_S {
            return;
        }
        self.block_started = Some(inputs.now);
        if let Some(summary) = summarize(&self.block, inputs.refresh_hz) {
            self.block_ready = Some(Block { summary, limit });
        }
        self.block.clear();
    }

    /// What both surfaces show.
    pub fn report(&self, inputs: &Inputs, wants: &Wants) -> Report {
        let now = inputs.now;
        let limit = self.classifier.confirmed();
        let testing = self.trial.as_ref().map(|t| Testing { kind: t.kind, seconds_left: t.seconds_left(now), side: t.side(now) });
        let frames = self.since(now, crate::classify::WINDOW_S);
        let right_now = summarize(&frames, inputs.refresh_hz).filter(|_| inputs.scene).map(|s| Now {
            real_fps: s.real_fps,
            refresh_hz: inputs.refresh_hz,
            filled_share: s.filled_share,
            late_cpu_share: s.late_cpu_share,
            late_gpu_share: s.late_gpu_share,
            app_cpu_ms: s.app_cpu.median,
            app_gpu_ms: s.app_gpu.median,
        });
        let recent = self.last_result.filter(|(_, at)| now - at < RESULT_LINE_S).map(|(line, _)| line);
        let line = if !inputs.scene {
            Line::NoScene
        } else if testing.is_some() {
            Line::Testing
        } else if let Some(line) = recent {
            line
        } else if self.left_alone == Some(LeftAlone::HandChanged) {
            Line::HandChanged
        } else if self.left_alone == Some(LeftAlone::AnotherApp) {
            Line::AnotherApp
        } else if self.left_alone == Some(LeftAlone::Refused) {
            Line::Refused
        } else if inputs.level == Level::Off {
            Line::Watching
        } else if inputs.level >= Level::Picture && limit == Limit::Processor {
            Line::ProcessorStays
        } else if inputs.level >= Level::Background && matches!(limit, Limit::Processor | Limit::Both) && inputs.listed_cores < MIN_LISTED_CORES {
            Line::NothingCompeting
        } else {
            Line::Limit
        };
        Report {
            limit,
            line,
            now: right_now,
            testing,
            comparisons: self.comparisons.clone(),
            guide: self.guide.clone(),
            wants: *wants,
            flags_seen: self.flags_seen,
            left_alone: self.left_alone,
        }
    }
}

/// Which change of SteamVR's value without the booster doing it means another app: the first time could be a person, the
/// second and after something that keeps setting it, like a dynamic resolution tool.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ValueWatch {
    last: Option<f32>,
    changes: u32,
}

impl ValueWatch {
    /// A value read while the booster owns nothing there. Whether another app is now judged to be controlling it.
    pub fn see(&mut self, value: f32) -> bool {
        if let Some(last) = self.last {
            if (last - value).abs() > crate::journal::FLOAT_SAME {
                self.changes += 1;
            }
        }
        self.last = Some(value);
        self.changes >= 2
    }

    /// The booster changed it itself: what it reads next is not news.
    pub fn forget_last(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::{LATE_CPU, LATE_GPU};

    const HZ: f32 = 72.0;
    const WORLD: u64 = 7;

    /// A pretend session: time passes in steps of a fifth of a second, as the overlay's reads do, and the frames say what
    /// the test wants them to.
    struct Run {
        engine: Engine,
        inputs: Inputs,
        index: u32,
        /// What frames look like, given what is in force.
        frames: fn(&Wants) -> (f64, u32, f32),
        wants: Wants,
    }

    impl Run {
        fn new(level: Level, frames: fn(&Wants) -> (f64, u32, f32)) -> Self {
            let inputs = Inputs { now: 0.0, scene: true, world: WORLD, people: Some(10), level, refresh_hz: HZ, listed_cores: 2.0, ..Inputs::default() };
            Self { engine: Engine::new(), inputs, index: 0, frames, wants: Wants::default() }
        }

        fn step(&mut self) {
            let (fps, flags, gpu_ms) = (self.frames)(&self.wants);
            let count = (fps * 0.2).round() as u32;
            let batch: Vec<Frame> = (0..count)
                .map(|i| {
                    self.index += 1;
                    Frame {
                        index: self.index,
                        time_s: self.inputs.now + i as f64 / fps,
                        presents: 2,
                        flags,
                        interval_ms: (1000.0 / fps) as f32,
                        pre_submit_gpu_ms: gpu_ms,
                        poses_ready_ms: 1.0,
                        // The processor's time is long only where these frames say the processor is what is late.
                        frame_ready_ms: if flags & LATE_CPU != 0 { 15.0 } else { 7.0 },
                        ..Frame::default()
                    }
                })
                .collect();
            if self.inputs.scene {
                self.engine.push(&self.inputs, &batch);
            }
            self.wants = self.engine.tick(&self.inputs, None);
            self.inputs.now += 0.2;
        }

        fn run(&mut self, seconds: f64) {
            for _ in 0..(seconds / 0.2) as u32 {
                self.step();
            }
        }

        fn report(&self) -> Report {
            self.engine.report(&self.inputs, &self.wants)
        }
    }

    /// The processor is late unless the programs are lowered, when frames really rise.
    fn programs_help(wants: &Wants) -> (f64, u32, f32) {
        if wants.lower_programs {
            (40.0, LATE_CPU, 8.0)
        } else {
            (36.0, LATE_CPU, 8.0)
        }
    }

    fn programs_do_nothing(_: &Wants) -> (f64, u32, f32) {
        (36.0, LATE_CPU, 8.0)
    }

    /// The graphics card is late, and lower sharpness truly lowers its time and raises frames.
    fn sharpness_helps(wants: &Wants) -> (f64, u32, f32) {
        if wants.sharpness < 1.0 {
            (42.0, LATE_GPU, 9.0)
        } else {
            (36.0, LATE_GPU, 12.0)
        }
    }

    /// The graphics card is late, and its time does not move: VRChat did not take the change.
    fn sharpness_not_taken(_: &Wants) -> (f64, u32, f32) {
        (36.0, LATE_GPU, 12.0)
    }

    #[test]
    fn off_changes_nothing_and_still_watches() {
        let mut run = Run::new(Level::Off, programs_help);
        run.run(120.0);
        assert_eq!(run.wants, Wants::default());
        let report = run.report();
        assert_eq!(report.limit, Limit::Processor, "it still says what the limit is");
        assert_eq!(report.line, Line::Watching);
        assert!(report.now.is_some());
        assert!(report.comparisons.is_empty(), "no comparison, so no gain");
    }

    #[test]
    fn step_one_tests_then_keeps_a_real_gain() {
        let mut run = Run::new(Level::Background, programs_help);
        run.run(JOIN_WAIT_S + 20.0);
        let testing = run.report().testing.expect("a comparison started");
        assert_eq!(testing.kind, Kind::Programs);
        // It alternates, so that the same scene gives both sides.
        run.run(5.0);
        assert!(!run.wants.lower_programs, "the first block is without");
        run.run(15.0);
        assert!(run.wants.lower_programs, "the second with");
        run.run(45.0);
        let report = run.report();
        assert!(report.testing.is_none());
        assert_eq!(report.line, Line::Kept);
        assert!(run.wants.lower_programs, "kept");
        let c = report.comparisons[0];
        assert_eq!(c.verdict, Verdict::Kept);
        assert!(c.after_fps > c.before_fps * 1.05, "{c:?}");
    }

    #[test]
    fn step_one_puts_back_what_gained_nothing_and_says_so() {
        let mut run = Run::new(Level::Background, programs_do_nothing);
        run.run(JOIN_WAIT_S + 20.0 + 65.0);
        let report = run.report();
        assert_eq!(report.line, Line::NoGain);
        assert!(!run.wants.lower_programs);
        assert_eq!(report.comparisons[0].verdict, Verdict::NoGain);
        // And it does not try again at once.
        run.run(60.0);
        assert!(run.report().testing.is_none());
        assert!(!run.wants.lower_programs);
    }

    #[test]
    fn a_quiet_pc_is_told_so_and_nothing_is_tested() {
        let mut run = Run::new(Level::Background, programs_help);
        run.inputs.listed_cores = 0.2;
        run.run(120.0);
        let report = run.report();
        assert!(report.testing.is_none() && report.comparisons.is_empty());
        assert_eq!(report.line, Line::NothingCompeting);
        assert!(!run.wants.lower_programs);
    }

    /// Lowering is for a processor limit. Limited by the graphics card, programs stay as they are.
    #[test]
    fn programs_are_not_lowered_for_a_graphics_card_limit() {
        let mut run = Run::new(Level::Background, sharpness_not_taken);
        run.run(120.0);
        assert!(run.report().comparisons.is_empty());
        assert!(!run.wants.lower_programs);
        assert!(run.wants.pace_panels, "its own panels are paced instead");
        assert_eq!(run.wants.sharpness, 1.0, "sharpness is step 2's");
    }

    /// Step 2, limited by the processor: sharpness does not change, and it says why.
    #[test]
    fn a_processor_limit_never_touches_sharpness() {
        let mut run = Run::new(Level::Picture, programs_do_nothing);
        run.run(200.0);
        assert_eq!(run.wants.sharpness, 1.0);
        assert!(run.report().comparisons.iter().all(|c| c.kind != Kind::Sharpness));
        assert_eq!(run.report().line, Line::ProcessorStays);
    }

    #[test]
    fn step_two_tests_sharpness_and_keeps_a_real_gain() {
        let mut run = Run::new(Level::Picture, sharpness_helps);
        run.run(JOIN_WAIT_S + 20.0 + 65.0);
        let report = run.report();
        assert_eq!(report.comparisons[0].kind, Kind::Sharpness);
        assert_eq!(report.line, Line::Kept);
        assert!((run.wants.sharpness - 0.85).abs() < 0.001);
        assert!(run.wants.panels_light);
    }

    #[test]
    fn sharpness_that_did_not_reach_vrchat_is_put_back_and_said() {
        let mut run = Run::new(Level::Picture, sharpness_not_taken);
        run.run(JOIN_WAIT_S + 20.0 + 65.0);
        let report = run.report();
        assert_eq!(report.line, Line::NotTaken);
        assert_eq!(run.wants.sharpness, 1.0);
        // Not tested again in this world.
        run.run(RETEST_S + 10.0);
        assert_eq!(run.report().comparisons.len(), 1);
    }

    #[test]
    fn one_sharpness_test_a_world_and_ten_minutes_between() {
        let mut run = Run::new(Level::Picture, sharpness_not_taken);
        run.run(JOIN_WAIT_S + 20.0 + 65.0);
        assert!(run.report().testing.is_none());
        // Another world soon after: no test, as it is under ten minutes.
        run.inputs.world = WORLD + 1;
        run.run(JOIN_WAIT_S + 60.0);
        assert!(run.report().testing.is_none());
        // Past the ten minutes, the new world may.
        run.run(RETEST_S);
        assert!(run.engine.sharpness_tested_in == Some(WORLD + 1));
    }

    #[test]
    fn leaving_the_scene_or_the_world_puts_everything_back() {
        let mut run = Run::new(Level::Picture, sharpness_helps);
        run.run(JOIN_WAIT_S + 20.0 + 65.0);
        assert!(run.wants.sharpness < 1.0);
        run.inputs.scene = false;
        run.step();
        assert_eq!(run.wants, Wants::default());
        assert_eq!(run.report().line, Line::NoScene);
        run.inputs.scene = true;
        run.step();
        assert_eq!(run.wants.sharpness, 1.0, "judged again, not back at once");

        let mut run = Run::new(Level::Picture, sharpness_helps);
        run.run(JOIN_WAIT_S + 20.0 + 65.0);
        run.inputs.world = WORLD + 1;
        run.step();
        assert_eq!(run.wants.sharpness, 1.0);
    }

    #[test]
    fn moving_the_slider_to_off_puts_everything_back_at_once() {
        let mut run = Run::new(Level::Picture, sharpness_helps);
        run.run(JOIN_WAIT_S + 20.0 + 65.0);
        assert!(run.wants.sharpness < 1.0 && run.wants.stop_preparing);
        run.inputs.level = Level::Off;
        run.step();
        assert_eq!(run.wants, Wants::default());
    }

    #[test]
    fn a_comparison_stops_when_the_scene_stops_being_the_same() {
        for spoil in [
            |i: &mut Inputs| i.people = Some(20),
            |i: &mut Inputs| i.dashboard = true,
            |i: &mut Inputs| i.recording = true,
            |i: &mut Inputs| i.world = WORLD + 1,
        ] {
            let mut run = Run::new(Level::Background, programs_help);
            run.run(JOIN_WAIT_S + 25.0);
            assert!(run.report().testing.is_some());
            spoil(&mut run.inputs);
            run.step();
            let report = run.report();
            assert!(report.testing.is_none());
            assert!(report.comparisons.is_empty(), "no gain from a spoiled comparison");
            assert!(!run.wants.lower_programs);
        }
    }

    #[test]
    fn test_anyway_works_at_off_and_keeps_nothing() {
        let mut run = Run::new(Level::Off, sharpness_helps);
        run.run(5.0);
        run.engine.request(&run.inputs.clone(), Request::TestAnyway(Kind::Sharpness));
        run.run(66.0);
        let report = run.report();
        assert_eq!(report.comparisons[0].verdict, Verdict::Kept);
        assert_eq!(run.wants.sharpness, 1.0, "it told, and the slider is still Off");
    }

    #[test]
    fn a_test_asked_for_at_a_bad_moment_says_so() {
        let mut run = Run::new(Level::Off, sharpness_helps);
        run.inputs.dashboard = true;
        run.run(1.0);
        run.engine.request(&run.inputs.clone(), Request::TestAnyway(Kind::Sharpness));
        assert_eq!(run.report().line, Line::CannotTestNow);
        assert!(run.report().testing.is_none());
    }

    #[test]
    fn a_hand_change_stops_sharpness_being_touched() {
        let mut run = Run::new(Level::Picture, sharpness_helps);
        run.run(JOIN_WAIT_S + 20.0 + 65.0);
        assert!(run.wants.sharpness < 1.0);
        run.engine.leave_sharpness_alone(LeftAlone::HandChanged);
        run.run(RESULT_LINE_S + 1.0);
        assert_eq!(run.wants.sharpness, 1.0);
        assert_eq!(run.report().line, Line::HandChanged);
        run.run(RETEST_S + 10.0);
        assert!(run.report().testing.is_none(), "not tested again");
    }

    #[test]
    fn another_app_is_recognised_by_changing_it_twice() {
        let mut watch = ValueWatch::default();
        assert!(!watch.see(1.0));
        assert!(!watch.see(1.0));
        assert!(!watch.see(0.9), "once could be a person");
        assert!(watch.see(1.1), "twice is something that keeps setting it");
        let mut mine = ValueWatch::default();
        mine.see(1.0);
        mine.forget_last();
        assert!(!mine.see(0.85), "the booster's own change is not news");
    }

    #[test]
    fn the_guided_list_measures_thirty_seconds_each_side() {
        let mut run = Run::new(Level::Avatars, programs_do_nothing);
        run.run(JOIN_WAIT_S + 20.0);
        let report = run.report();
        assert_eq!(report.guide.item(), Some(guided::Item::MaxShown));
        assert!(report.guide.likely);
        run.engine.request(&run.inputs.clone(), Request::GuidedDone);
        run.run(1.0);
        assert!(run.report().guide.measuring.is_some());
        run.run(HAND_S);
        let report = run.report();
        assert!(report.guide.measuring.is_none());
        let m = report.guide.results[0];
        assert_eq!(m.item, guided::Item::MaxShown);
        assert!(m.before_fps > 30.0 && m.after_fps > 30.0, "{m:?}");
        // A/B: a second Done gives another figure beside the first.
        run.engine.request(&run.inputs.clone(), Request::GuidedDone);
        run.run(HAND_S + 1.0);
        assert_eq!(run.report().guide.results.len(), 2);
        run.engine.request(&run.inputs.clone(), Request::GuidedNext);
        assert_eq!(run.engine.guide.item(), Some(guided::Item::BlockVeryPoor));
    }

    /// Fine or unsure: it says nothing's likely to help, and the list is still there to open.
    #[test]
    fn the_guided_list_says_when_it_is_unlikely_to_help() {
        fn fine(_: &Wants) -> (f64, u32, f32) {
            (72.0, 0, 5.0)
        }
        let mut run = Run::new(Level::Avatars, fine);
        run.run(60.0);
        let report = run.report();
        assert_eq!(report.limit, Limit::NotSure, "these test frames are all presented twice");
        assert!(!report.guide.likely);
        assert!(!report.guide.items.is_empty());
    }

    #[test]
    fn a_held_test_change_goes_with_the_scene_too() {
        let mut run = Run::new(Level::Off, programs_do_nothing);
        run.run(1.0);
        run.engine.request(&run.inputs.clone(), Request::HoldTestChange(true));
        run.step();
        assert!((run.wants.sharpness - SHARPNESS_STEP).abs() < 0.001, "held whatever the limit");
        run.inputs.scene = false;
        run.step();
        assert_eq!(run.wants.sharpness, 1.0);
    }

    /// The log's own blocks: one every five seconds of frames, whatever the level.
    #[test]
    fn blocks_for_the_log_come_every_five_seconds_at_off_too() {
        let mut run = Run::new(Level::Off, programs_do_nothing);
        let mut blocks = 0;
        for _ in 0..(60.0 / 0.2) as u32 {
            run.step();
            if run.engine.take_block().is_some() {
                blocks += 1;
            }
        }
        assert!((10..=12).contains(&blocks), "{blocks}");
    }
}
