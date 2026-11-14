//! Live translation without SteamVR: each look's lines read, placed and followed ([`crate::pipeline`],
//! [`crate::track`]), the lines that need it sent to the translator in batches on a thread of its own (a slow model
//! never holds the reading back), translations cached by text, painted, and handed out as the quads to show now.

use crate::geometry::{self, V3};
use crate::language;
use crate::paint::{Painted, Painter};
use crate::pipeline::{self, Engine, Look, Timings};
use crate::provider::{Batch, Cache, Failure, Item, Translator};
use crate::track::{Colours, Track, Tracker};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::Arc;

/// Translations shown at once, at most: the most central and nearest lines first.
pub const MOST_SHOWN: usize = 20;
/// Lines asked for in one request, the most central first. Small, so that a line looked at never waits behind a long
/// batch: Enzo's log of 2026-11-14 had batches of 24 lines taking 21 s on the built-in translator, and the line he looked
/// at waited for the whole batch and for its own after it. The built-in translator reads them together in one pass
/// ([`crate::builtin::Builtin::lines`]) and hands each out as it is done.
pub const MOST_IN_BATCH: usize = 6;

/// Whom the translations are for.
#[derive(Clone, Debug, PartialEq)]
pub struct Reader {
    /// The language to translate into (BCP 47).
    pub target: String,
    /// Languages also read: lines in them are left as they are.
    pub reads: Vec<String>,
}

/// A translation to show: where (the original line's place in the room), how large, and its picture.
#[derive(Clone, Debug)]
pub struct Quad {
    pub track: u64,
    pub pose: geometry::M34,
    pub width: f32,
    pub height: f32,
    pub picture: Arc<Painted>,
}

/// What the translation is doing, for the menu's status line.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Activity {
    /// Looking; nothing to translate in view.
    #[default]
    Looking,
    /// The translator is getting ready (a server loading its model).
    Waking,
    /// Lines are waiting for the translator.
    Translating(usize),
    /// The translator failed: the last reason.
    Failed(Failure),
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub quads: Vec<Quad>,
    pub activity: Activity,
    pub timings: Timings,
    /// Lines followed now (shown or not).
    pub lines: usize,
    /// How long the batch on its way has been waited for, seconds.
    pub waiting_s: Option<f32>,
    /// This look found lines again elsewhere than the anchors had them: all lines moved by this (anchor space), after
    /// turning by `turned` (radians about the vertical through the head, when the error was a turn's).
    pub moved: Option<V3>,
    pub turned: Option<f32>,
}

/// How long a line took to show, seconds after the picture it was first read in was taken: read (recognised and
/// placed), its translation there, its translation shown. `known`: translated before it was read (the cache).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chain {
    pub read: f32,
    pub translated: f32,
    pub shown: f32,
    pub known: bool,
    /// Its angle off where the head looked when it showed, degrees.
    pub off_deg: f32,
}

struct Request {
    target: String,
    keys: Vec<String>,
    batch: Batch,
    asked: std::time::Instant,
}

enum FromTranslator {
    /// Ready, after warming up this long.
    Warm(std::time::Duration),
    /// One line of a request translated (its key), before the others are.
    Line(String, String, String),
    /// The request done: how, how long it took, and how long its first line took.
    Answer(Request, Result<(), Failure>, std::time::Duration, Option<std::time::Duration>),
}

/// The translator on its own thread: requests in, answers out.
struct TranslatorThread {
    send: mpsc::Sender<Request>,
    answers: mpsc::Receiver<FromTranslator>,
}

impl TranslatorThread {
    fn start(mut translator: Box<dyn Translator>) -> Self {
        let (send, requests) = mpsc::channel::<Request>();
        let (give, answers) = mpsc::channel();
        std::thread::Builder::new()
            .name("translate requests".into())
            .spawn(move || {
                let t = std::time::Instant::now();
                translator.warm_up();
                if give.send(FromTranslator::Warm(t.elapsed())).is_err() {
                    return;
                }
                for r in requests {
                    let t = std::time::Instant::now();
                    let mut first = None;
                    let answer = translator.translate_each(&r.batch, &mut |i, text| {
                        first.get_or_insert(t.elapsed());
                        if let Some(k) = r.keys.get(i) {
                            let _ = give.send(FromTranslator::Line(r.target.clone(), k.clone(), text));
                        }
                    });
                    if give.send(FromTranslator::Answer(r, answer, t.elapsed(), first)).is_err() {
                        break;
                    }
                }
            })
            .expect("a thread for the translator");
        Self { send, answers }
    }
}

pub struct Core {
    pub engine: Option<Engine>,
    pub tracker: Tracker,
    cache: Cache,
    painter: Painter,
    translator: TranslatorThread,
    in_flight: HashSet<(String, String)>,
    /// Lines the translator failed on, and when: asked again after a while.
    failed: HashMap<(String, String), f64>,
    last_failure: Option<Failure>,
    painted: HashMap<u64, (String, Colours, Arc<Painted>)>,
    pub reader: Reader,
    /// The translator has warmed up.
    warm: bool,
    /// When the batch on its way was sent.
    asked_at: Option<std::time::Instant>,
    /// When a line to translate was first followed, and whether a translation was shown since (for the log).
    first_seen: Option<std::time::Instant>,
    first_shown: bool,
    /// Lines for translate.log: what batches cost, and how long the first translation took. Taken by the caller.
    pub log: Vec<String>,
    /// Also what was read and translated (development builds only: the text seen in VR is the person's).
    pub log_text: bool,
    /// Answers taken from the translator so far.
    answered: u64,
    /// The lines shown by the last scene: they keep their place among the [`MOST_SHOWN`] while they are about as central
    /// as the others (a head moving a little must not swap translations in and out).
    shown: HashSet<u64>,
    /// The clock the times here are on; [`Core::step`]'s `now` must be on it too (the caller may set it to its own).
    pub clock: std::time::Instant,
    /// Where the head looked last: the lines nearest it are translated first.
    gaze: Option<(V3, V3)>,
    /// For each line followed: when the picture it was first read in was taken, and when it was read (clock seconds).
    born: HashMap<u64, (f64, f64)>,
    /// When the look being followed was read (set by [`Core::follow`] for [`Core::after_look`]).
    read_at: Option<f64>,
    /// When each translation arrived, by key (clock seconds).
    arrived: HashMap<String, f64>,
    /// Lines shown at least once.
    ever_shown: HashSet<u64>,
    /// The chains of the lines shown for the first time since the caller last took them.
    pub chains: Vec<Chain>,
    /// A line newly followed is translated again even when its text was translated before (development builds, to
    /// measure the translator on texts seen before).
    pub no_cache: bool,
    /// Where translations are kept between sessions, whether some arrived since they were written, and when.
    cache_file: Option<std::path::PathBuf>,
    cache_changed: bool,
    cache_saved: std::time::Instant,
}

impl Core {
    pub fn new(engine: Option<Engine>, translator: Box<dyn Translator>, reader: Reader) -> Self {
        Self {
            engine,
            tracker: Tracker::default(),
            cache: Cache::default(),
            painter: Painter::default(),
            translator: TranslatorThread::start(translator),
            in_flight: HashSet::new(),
            failed: HashMap::new(),
            last_failure: None,
            painted: HashMap::new(),
            reader,
            warm: false,
            asked_at: None,
            first_seen: None,
            first_shown: false,
            log: Vec::new(),
            log_text: false,
            answered: 0,
            shown: HashSet::new(),
            clock: std::time::Instant::now(),
            gaze: None,
            born: HashMap::new(),
            read_at: None,
            arrived: HashMap::new(),
            ever_shown: HashSet::new(),
            chains: Vec::new(),
            no_cache: false,
            cache_file: None,
            cache_changed: false,
            cache_saved: std::time::Instant::now(),
        }
    }

    /// Translations kept in `path` from earlier sessions, and new ones kept there ([`Core::save_cache`]).
    pub fn keep_cache(&mut self, path: std::path::PathBuf) {
        self.cache = Cache::load(&path);
        self.cache_file = Some(path);
    }

    /// Writes the translations that arrived since the last time, at most every 30 s unless `now`.
    pub fn save_cache(&mut self, now: bool) {
        if !self.cache_changed || !(now || self.cache_saved.elapsed() >= std::time::Duration::from_secs(30)) {
            return;
        }
        if let Some(path) = &self.cache_file {
            if let Err(e) = self.cache.save(path) {
                self.log.push(format!("translations not kept: {e}")); // i18n: not shown (a log's line)
            }
        }
        self.cache_changed = false;
        self.cache_saved = std::time::Instant::now();
    }

    /// Another translator (the person chose another provider): what was translated stays.
    pub fn set_translator(&mut self, translator: Box<dyn Translator>) {
        self.translator = TranslatorThread::start(translator);
        self.in_flight.clear();
        self.failed.clear();
        self.last_failure = None;
        self.warm = false;
        self.asked_at = None;
    }

    pub fn set_reader(&mut self, reader: Reader) {
        if reader != self.reader {
            self.reader = reader;
            self.painted.clear();
            self.in_flight.clear();
        }
    }

    /// One look: read, place, follow, ask, paint. `gaze` is where the head looks now (origin, direction), to choose
    /// which translations to show when there are too many.
    pub fn step(&mut self, now: f64, look: &Look, gaze: (V3, V3)) -> anyhow::Result<Scene> {
        let engine = self.engine.as_mut().ok_or_else(|| anyhow::anyhow!("the text reader is not loaded"))?; // i18n: not shown (log detail)
        let (lines, timings) = engine.read(look)?;
        Ok(self.follow(now, look, lines, timings, gaze))
    }

    /// The part of a step after the text is read (the caller may read with the engine taken out of the core, so that
    /// translations arriving meanwhile show): place, follow, ask, paint. `now` is when the look's picture was taken.
    pub fn follow(&mut self, now: f64, look: &Look, lines: Vec<crate::ocr::Line>, timings: Timings, gaze: (V3, V3)) -> Scene {
        let read_at = self.clock.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        if self.log_text {
            self.log.push(format!("read: {:?}", lines.iter().map(|l| (l.text.as_str(), (l.confidence * 100.0).round())).collect::<Vec<_>>())); // i18n: not shown (a log's line)
        }
        let lines: Vec<_> = lines.into_iter().filter(|l| language::needs_translating(&l.text, &self.reader.target, &self.reader.reads)).collect();
        let sightings = pipeline::sightings(look, &lines);
        self.tracker.head = Some(gaze.0);
        let locked = self.tracker.head_locked.len();
        let followed = self.tracker.update(now, sightings, &|track: &Track| pipeline::seen(look, track));
        if self.log_text {
            let at = |t: &Track| t.placed.centre().map(|v| (v * 100.0).round() / 100.0);
            self.log.push(format!("followed: {:?}", self.tracker.tracks.iter().map(|t| (t.text.as_str(), at(t), t.placed.measured, t.sightings)).collect::<Vec<_>>())); // i18n: not shown (a log's line)
        }
        if self.tracker.head_locked.len() > locked {
            self.log.push(format!("{} text(s) found to move with the head: left alone for a minute", self.tracker.head_locked.len() - locked)); // i18n: not shown (a log's line)
        }
        if followed.held {
            self.log.push(format!(
                "found lines again {:.0} cm and {:.1} degrees from their anchors, {} agreeing (not moved: anchors held)", // i18n: not shown (a log's line)
                geometry::norm(followed.moved.unwrap_or_default()) * 100.0,
                followed.turned.unwrap_or(0.0).to_degrees(),
                followed.agreeing
            ));
        }
        self.read_at = Some(read_at);
        let mut timings = timings;
        timings.place = t.elapsed().as_secs_f32() * 1000.0;
        let mut scene = self.after_look(now, gaze, timings);
        if !followed.held && (followed.moved.is_some() || followed.turned.is_some()) {
            scene.moved = Some(followed.moved.unwrap_or_default());
            scene.turned = followed.turned;
        }
        scene
    }

    /// The part of a step after the lines are followed: ask, collect, paint, choose. Tests call it after feeding the
    /// tracker themselves.
    pub fn after_look(&mut self, now: f64, gaze: (V3, V3), timings: Timings) -> Scene {
        self.gaze = Some(gaze);
        let read_at = self.read_at.take().unwrap_or(now);
        for t in &self.tracker.tracks {
            if let std::collections::hash_map::Entry::Vacant(e) = self.born.entry(t.id) {
                e.insert((now, read_at));
                if self.no_cache {
                    self.cache.remove(&self.reader.target, &t.key);
                }
            }
        }
        self.collect(now);
        self.ask(now);
        let live: HashSet<u64> = self.tracker.tracks.iter().map(|t| t.id).collect();
        self.painted.retain(|id, _| live.contains(id));
        self.born.retain(|id, _| live.contains(id));
        self.ever_shown.retain(|id| live.contains(id));
        if self.arrived.len() > 4 * MOST_SHOWN + self.tracker.tracks.len() {
            let keys: HashSet<&String> = self.tracker.tracks.iter().map(|t| &t.key).collect();
            self.arrived.retain(|k, _| keys.contains(k));
        }
        let target = self.reader.target.clone();
        let mut quads = Vec::new();
        let mut order: Vec<&Track> = self.tracker.tracks.iter().filter(|t| t.settled()).collect();
        let shown = &self.shown;
        let priority = |t: &Track| {
            let p = gaze_priority(t, gaze);
            if shown.contains(&t.id) {
                0.6 * p
            } else {
                p
            }
        };
        order.sort_by(|a, b| priority(a).total_cmp(&priority(b)));
        let mut wanted = Vec::new();
        for t in order {
            if wanted.len() >= MOST_SHOWN {
                break;
            }
            // A clearer read of a line shown asks for its translation anew: meanwhile the one shown stays.
            let Some(translation) = self.cache.get(&target, &t.key).or_else(|| self.painted.get(&t.id).map(|p| &p.0)) else { continue };
            if same_text(translation, &t.text) {
                continue;
            }
            wanted.push((t.id, translation.clone(), t.colours, t.placed));
        }
        for (id, translation, colours, placed) in wanted {
            let fresh = match self.painted.get(&id) {
                Some((text, c, _)) => text != &translation || colours_far(*c, colours),
                None => true,
            };
            if fresh {
                let painted = Arc::new(self.painter.paint(&translation, colours, placed.width, placed.height));
                self.painted.insert(id, (translation.clone(), colours, painted));
            }
            let picture = self.painted[&id].2.clone();
            quads.push(Quad { track: id, pose: placed.pose, width: picture.quad_width, height: picture.quad_height, picture });
        }
        self.shown = quads.iter().map(|q| q.track).collect();
        let shown_at = self.clock.elapsed().as_secs_f64();
        for q in &quads {
            if self.ever_shown.insert(q.track) {
                if let (Some(&(taken, read)), Some(t)) = (self.born.get(&q.track), self.tracker.get(q.track)) {
                    let arrived = self.arrived.get(&t.key).copied().unwrap_or(taken);
                    let to = geometry::sub(t.placed.centre(), gaze.0);
                    let off_deg = geometry::dot(geometry::scale(to, 1.0 / geometry::norm(to).max(0.01)), gaze.1).clamp(-1.0, 1.0).acos().to_degrees();
                    self.chains.push(Chain { read: (read - taken) as f32, translated: (arrived - taken) as f32, shown: (shown_at - taken) as f32, known: arrived <= taken, off_deg });
                }
            }
        }
        let waiting = self.in_flight.len() + self.tracker.tracks.iter().filter(|t| self.cache.get(&target, &t.key).is_none()).count().min(1);
        if self.first_seen.is_none() && self.tracker.tracks.iter().any(|t| self.cache.get(&target, &t.key).is_none()) {
            self.first_seen = Some(std::time::Instant::now());
        }
        if !self.first_shown && !quads.is_empty() {
            self.first_shown = true;
            if let Some(seen) = self.first_seen {
                self.log.push(format!("first translation shown {:.1} s after its line was first followed", seen.elapsed().as_secs_f32())); // i18n: not shown (a log's line)
            }
        }
        let activity = match (&self.last_failure, waiting) {
            (None, n) if n > 0 && !self.warm => Activity::Waking,
            (Some(f), _) => Activity::Failed(f.clone()),
            (None, 0) => Activity::Looking,
            (None, n) => Activity::Translating(n),
        };
        Scene { quads, activity, timings, lines: self.tracker.tracks.len(), waiting_s: self.asked_at.map(|t| t.elapsed().as_secs_f32()), moved: None, turned: None }
    }

    /// Between looks: when the translator answered since, the quads to show now (a translation shows when it arrives,
    /// not at the next look, which can be seconds away while the game is short of processor time).
    pub fn poll(&mut self, now: f64, gaze: (V3, V3)) -> Option<Scene> {
        let before = self.answered;
        self.collect(now);
        (self.answered != before).then(|| self.after_look(now, gaze, Timings::default()))
    }

    fn collect(&mut self, now: f64) {
        while let Ok(message) = self.translator.answers.try_recv() {
            let (request, answer, took, first) = match message {
                FromTranslator::Warm(took) => {
                    self.warm = true;
                    self.log.push(format!("translator ready after {:.0} ms of warming up", took.as_secs_f64() * 1000.0)); // i18n: not shown (a log's line)
                    continue;
                }
                FromTranslator::Line(target, key, text) => {
                    // Shown now, before the rest of its batch.
                    self.answered += 1;
                    self.last_failure = None;
                    if self.log_text {
                        self.log.push(format!("into {target}: {key:?} -> {text:?}")); // i18n: not shown (a log's line)
                    }
                    self.in_flight.remove(&(target.clone(), key.clone()));
                    if target == self.reader.target {
                        self.arrived.insert(key.clone(), self.clock.elapsed().as_secs_f64());
                    }
                    self.cache.put(&target, &key, text);
                    self.cache_changed = true;
                    continue;
                }
                FromTranslator::Answer(r, a, took, first) => (r, a, took, first),
            };
            self.answered += 1;
            self.asked_at = None;
            self.log.push(format!(
                "batch of {} lines: {} in {:.0} ms, the first line after {:.0} ms ({:.0} ms queued behind the translator)", // i18n: not shown (a log's line)
                request.keys.len(),
                if answer.is_ok() { "translated" } else { "failed" }, // i18n: not shown (a log's line)
                took.as_secs_f64() * 1000.0,
                first.unwrap_or(took).as_secs_f64() * 1000.0,
                request.asked.elapsed().saturating_sub(took).as_secs_f64() * 1000.0
            ));
            for k in &request.keys {
                self.in_flight.remove(&(request.target.clone(), k.clone()));
            }
            match answer {
                Ok(()) => self.last_failure = None,
                Err(f) => {
                    for k in &request.keys {
                        if self.cache.get(&request.target, k).is_none() {
                            self.failed.insert((request.target.clone(), k.clone()), now);
                        }
                    }
                    self.last_failure = Some(f);
                }
            }
        }
    }

    /// The next lines to translate, when the translator is free: those nearest where the head looks first (Enzo looks
    /// at a sign and waits for it; the 2026-11-14 log asked in reading order, 24 at a time, with up to 215 lines
    /// followed).
    fn ask(&mut self, now: f64) {
        let target = self.reader.target.clone();
        // One request at a time: what is looked at by the time the translator is free goes first.
        if self.in_flight.iter().any(|(t, _)| *t == target) {
            return;
        }
        let mut todo: Vec<(f32, &Track)> = self
            .tracker
            .tracks
            .iter()
            .filter(|t| self.cache.get(&target, &t.key).is_none())
            .filter(|t| !self.failed.get(&(target.clone(), t.key.clone())).is_some_and(|at| now - at < 8.0))
            .map(|t| (self.gaze.map_or(0.0, |g| gaze_priority(t, g)) + if t.settled() { 0.0 } else { 0.1 }, t))
            .collect();
        todo.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut keys: Vec<String> = Vec::new();
        for (_, t) in todo {
            if !keys.contains(&t.key) {
                keys.push(t.key.clone());
            }
            if keys.len() >= MOST_IN_BATCH {
                break;
            }
        }
        if keys.is_empty() {
            return;
        }
        let batch = Batch { target: target.clone(), lines: keys.iter().map(|k| Item { text: k.clone(), source: language::guess(k).map(str::to_string) }).collect() };
        for k in &keys {
            self.in_flight.insert((target.clone(), k.clone()));
        }
        self.asked_at = Some(std::time::Instant::now());
        if self.translator.send.send(Request { target, keys, batch, asked: std::time::Instant::now() }).is_err() {
            self.in_flight.clear();
        }
    }

    /// A translation already known.
    pub fn cached(&self, text: &str) -> Option<&String> {
        self.cache.get(&self.reader.target, &crate::track::key(text))
    }
}

/// How much a line should wait behind others: how far from where the head looks (its angle off the gaze counts most),
/// then how far away. Lower first.
pub fn gaze_priority(t: &Track, gaze: (V3, V3)) -> f32 {
    let to = geometry::sub(t.placed.centre(), gaze.0);
    let d = geometry::norm(to).max(0.01);
    let off = 1.0 - geometry::dot(geometry::scale(to, 1.0 / d), gaze.1);
    off * 4.0 + d * 0.05
}

/// A translation equal to its source (ignoring case, spaces and punctuation) is not drawn.
pub fn same_text(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect::<String>();
    norm(a) == norm(b)
}

fn colours_far(a: Colours, b: Colours) -> bool {
    let d = |x: [u8; 3], y: [u8; 3]| (0..3).map(|i| (x[i] as i32 - y[i] as i32).abs()).max().unwrap_or(0);
    d(a.background, b.background) > 24 || d(a.text, b.text) > 40
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::Placed;
    use crate::track::{Seen, Sighting};

    struct Echo;
    impl Translator for Echo {
        fn translate(&mut self, batch: &Batch) -> Result<Vec<String>, Failure> {
            Ok(batch.lines.iter().map(|l| format!("T {}", l.text)).collect())
        }
    }

    /// A line `degrees` to the right of straight ahead, 3 m away.
    fn line(text: &str, degrees: f32) -> Sighting {
        let a = degrees.to_radians();
        let mut pose = geometry::rotation([0.0, 1.0, 0.0], -a, [0.0; 3]);
        pose[0][3] = 3.0 * a.sin();
        pose[1][3] = 1.6;
        pose[2][3] = -3.0 * a.cos();
        Sighting { text: text.into(), confidence: 0.95, placed: Placed { pose, width: 0.3, height: 0.06, measured: true, distance: 3.0 }, colours: Colours { background: [250; 3], text: [0; 3] }, context: vec![], in_eye: [0.0; 3] }
    }

    fn gaze(degrees: f32) -> (V3, V3) {
        let a = degrees.to_radians();
        ([0.0, 1.6, 0.0], [a.sin(), 0.0, -a.cos()])
    }

    /// Until every line followed is translated (the translator answers on its own thread).
    fn settle(core: &mut Core, lines: &dyn Fn() -> Vec<Sighting>, at: f32) -> Scene {
        for k in 0..200 {
            core.tracker.update(k as f64 * 0.01, lines(), &|_| Seen::Open);
            let scene = core.after_look(k as f64 * 0.01, gaze(at), Default::default());
            if core.tracker.tracks.iter().all(|t| core.cached(&t.text).is_some()) && k > 2 {
                return scene;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("never translated");
    }

    #[test]
    fn a_little_head_movement_does_not_swap_the_translations_shown() {
        // More lines than are shown (in one batch, so that all come at once), one every 3 degrees.
        let texts: Vec<String> = (0..22).map(|i| format!("線{i}")).collect();
        let lines = || texts.iter().enumerate().map(|(i, t)| line(t, i as f32 * 3.0 - 31.5)).collect::<Vec<_>>();
        let mut core = Core::new(None, Box::new(Echo), Reader { target: "en".into(), reads: vec![] });
        let first = settle(&mut core, &lines, 0.0);
        assert_eq!(first.quads.len(), MOST_SHOWN);
        let shown: HashSet<u64> = first.quads.iter().map(|q| q.track).collect();
        // The head turns 4 degrees: the lines at the edge are hardly more or less central than before.
        let after = core.after_look(10.0, gaze(4.0), Default::default());
        let now: HashSet<u64> = after.quads.iter().map(|q| q.track).collect();
        assert_eq!(now, shown, "the same translations stay");
        // Turned well away, the ones now central take over.
        let far = core.after_look(10.5, gaze(40.0), Default::default());
        let now: HashSet<u64> = far.quads.iter().map(|q| q.track).collect();
        assert_ne!(now, shown);
    }

    #[test]
    fn a_clearer_read_keeps_the_translation_shown_until_its_own_comes() {
        let mut core = Core::new(None, Box::new(Echo), Reader { target: "en".into(), reads: vec![] });
        let first = settle(&mut core, &|| vec![line("出口はこちら", 0.0)], 0.0);
        assert_eq!(first.quads.len(), 1);
        let id = first.quads[0].track;
        // A clearer read of the same line (one letter more): asked anew; meanwhile the old translation stays.
        let mut better = line("出口はこちらです", 0.0);
        better.confidence = 0.99;
        core.tracker.update(5.0, vec![better], &|_| Seen::Open);
        assert_eq!(core.tracker.tracks[0].text, "出口はこちらです");
        let scene = core.after_look(5.0, gaze(0.0), Default::default());
        assert!(scene.quads.iter().any(|q| q.track == id), "no gap while the new translation is on its way");
    }

    /// A translator that takes `per_line` for each line, one after the other, handing each out when done, and says
    /// which lines it was asked for, in order.
    struct Slow {
        per_line: std::time::Duration,
        asked: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }
    impl Translator for Slow {
        fn translate(&mut self, batch: &Batch) -> Result<Vec<String>, Failure> {
            let mut out = Vec::new();
            self.translate_each(batch, &mut |_, t| out.push(t))?;
            Ok(out)
        }
        fn translate_each(&mut self, batch: &Batch, each: &mut dyn FnMut(usize, String)) -> Result<(), Failure> {
            self.asked.lock().unwrap().push(batch.lines.iter().map(|l| l.text.clone()).collect());
            for (i, l) in batch.lines.iter().enumerate() {
                std::thread::sleep(self.per_line);
                each(i, format!("T {}", l.text));
            }
            Ok(())
        }
    }

    /// Enzo's 2026-11-14 log: 40 to 215 lines followed, asked 24 at a time in reading order, 21 s a batch; the sign he
    /// looked at waited for whole batches before its own. The line looked at goes first, in a small batch, and shows
    /// as soon as it is translated.
    #[test]
    fn the_line_looked_at_is_translated_first_and_shows_at_once() {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut core = Core::new(None, Box::new(Slow { per_line: std::time::Duration::from_millis(150), asked: asked.clone() }), Reader { target: "en".into(), reads: vec![] });
        // Wait for the translator's thread to be ready.
        let texts: Vec<String> = (0..40).map(|i| format!("線{i}")).collect();
        // Lines from 60 degrees left to 57 right; the head looks at the one 21 degrees right ("線27").
        let lines = || texts.iter().enumerate().map(|(i, t)| line(t, i as f32 * 3.0 - 60.0)).collect::<Vec<_>>();
        let start = std::time::Instant::now();
        core.tracker.update(0.0, lines(), &|_| Seen::Open);
        let mut shown_after = None;
        for k in 0..400 {
            let scene = core.after_look(k as f64 * 0.01, gaze(21.0), Default::default());
            if scene.quads.iter().any(|q| core.tracker.get(q.track).is_some_and(|t| t.text == "線27")) {
                shown_after = Some(start.elapsed());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let shown_after = shown_after.expect("the line looked at is shown");
        let first = asked.lock().unwrap()[0].clone();
        assert!(first.len() <= MOST_IN_BATCH, "a small batch: {first:?}");
        assert_eq!(first[0], "線27", "the line looked at is asked for first: {first:?}");
        for near in ["線26", "線28"] {
            assert!(first.contains(&near.to_string()), "its neighbours come with it: {first:?}");
        }
        // One line's time, not a batch's (6 × 150 ms) nor all 40 lines' (6 s).
        assert!(shown_after < std::time::Duration::from_millis(600), "shown after {shown_after:?}");
        // Every line is translated in the end.
        for k in 0..1000 {
            core.after_look(5.0 + k as f64 * 0.01, gaze(21.0), Default::default());
            if core.tracker.tracks.iter().all(|t| core.cached(&t.text).is_some()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("not every line was translated");
    }

    /// A line turned to while a batch is on its way goes into the next batch, before the lines asked for earlier.
    #[test]
    fn a_line_turned_to_goes_before_those_waiting() {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut core = Core::new(None, Box::new(Slow { per_line: std::time::Duration::from_millis(40), asked: asked.clone() }), Reader { target: "en".into(), reads: vec![] });
        let texts: Vec<String> = (0..30).map(|i| format!("札{i}")).collect();
        let lines = || texts.iter().enumerate().map(|(i, t)| line(t, i as f32 * 4.0 - 60.0)).collect::<Vec<_>>();
        core.tracker.update(0.0, lines(), &|_| Seen::Open);
        core.after_look(0.0, gaze(-60.0), Default::default());
        // The head turns to the far right while the first batch (the far left) is being translated.
        for k in 0..400 {
            core.after_look(0.01 * k as f64, gaze(56.0), Default::default());
            if core.cached("札29").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let asked = asked.lock().unwrap().clone();
        assert!(asked[0].contains(&"札0".to_string()), "{asked:?}");
        assert_eq!(asked[1][0], "札29", "the line turned to is next: {asked:?}");
    }

    /// The chain's times are measured from the picture: read, translated, shown. A line translated before shows at once.
    #[test]
    fn the_chain_is_timed_and_a_known_line_shows_at_once() {
        let mut core = Core::new(None, Box::new(Echo), Reader { target: "en".into(), reads: vec![] });
        let scene = settle(&mut core, &|| vec![line("出口", 0.0)], 0.0);
        assert_eq!(scene.quads.len(), 1);
        let chains = std::mem::take(&mut core.chains);
        assert_eq!(chains.len(), 1, "one line shown for the first time: {chains:?}");
        assert!(!chains[0].known && chains[0].shown >= chains[0].translated && chains[0].translated >= 0.0, "{chains:?}");
        // The same text on another sign, later: from the cache, at the first look that reads it.
        // (Its place before the eye differs too: the same text at the same place before the eye is a notice that moves
        // with the head.)
        let mut other = line("出口", 40.0);
        other.in_eye = [1.0, 0.0, -2.0];
        core.tracker.update(10.0, vec![other], &|_| Seen::Away);
        let scene = core.after_look(10.0, gaze(40.0), Default::default());
        assert_eq!(scene.quads.len(), 1);
        let chains = std::mem::take(&mut core.chains);
        assert!(chains.len() == 1 && chains[0].known, "{chains:?}");
    }

    #[test]
    fn translations_are_kept_between_sessions() {
        let dir = std::env::temp_dir().join(format!("orbiters-translate-cache-{}", std::process::id()));
        let path = dir.join("translations.jsonl");
        let _ = std::fs::remove_file(&path);
        let mut core = Core::new(None, Box::new(Echo), Reader { target: "fr".into(), reads: vec![] });
        core.keep_cache(path.clone());
        settle(&mut core, &|| vec![line("西口", 0.0)], 0.0);
        core.save_cache(true);
        // Another session, a translator that never answers: the sign seen before shows from the first look.
        struct Never;
        impl Translator for Never {
            fn translate(&mut self, _: &Batch) -> Result<Vec<String>, Failure> {
                std::thread::sleep(std::time::Duration::from_secs(5));
                Err(Failure::Busy)
            }
        }
        let mut next = Core::new(None, Box::new(Never), Reader { target: "fr".into(), reads: vec![] });
        next.keep_cache(path.clone());
        assert_eq!(next.cached("西口").map(String::as_str), Some("T 西口"));
        next.tracker.update(0.0, vec![line("西口", 0.0)], &|_| Seen::Open);
        next.tracker.update(0.3, vec![line("西口", 0.0)], &|_| Seen::Open);
        assert_eq!(next.after_look(0.3, gaze(0.0), Default::default()).quads.len(), 1);
        // A damaged line is skipped, the rest read.
        std::fs::write(&path, "not json\n[\"fr\",\"東口\",\"Sortie est\"]\n").unwrap();
        let c = Cache::load(&path);
        assert_eq!(c.get("fr", "東口").map(String::as_str), Some("Sortie est"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
