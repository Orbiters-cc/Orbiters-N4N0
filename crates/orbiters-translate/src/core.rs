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
/// Lines asked for in one request.
const MOST_IN_BATCH: usize = 24;

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
    Answer(Request, Result<Vec<String>, Failure>, std::time::Duration),
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
                    let answer = translator.translate(&r.batch);
                    if give.send(FromTranslator::Answer(r, answer, t.elapsed())).is_err() {
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
        }
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
        let t = std::time::Instant::now();
        if self.log_text {
            self.log.push(format!("read: {:?}", lines.iter().map(|l| (l.text.as_str(), (l.confidence * 100.0).round())).collect::<Vec<_>>())); // i18n: not shown (a log's line)
        }
        let lines: Vec<_> = lines.into_iter().filter(|l| language::needs_translating(&l.text, &self.reader.target, &self.reader.reads)).collect();
        let sightings = pipeline::sightings(look, &lines);
        self.tracker.update(now, sightings, &|track: &Track| pipeline::seen(look, track));
        let mut timings = timings;
        timings.place = t.elapsed().as_secs_f32() * 1000.0;
        Ok(self.after_look(now, gaze, timings))
    }

    /// The part of a step after the lines are followed: ask, collect, paint, choose. Tests call it after feeding the
    /// tracker themselves.
    pub fn after_look(&mut self, now: f64, gaze: (V3, V3), timings: Timings) -> Scene {
        self.collect(now);
        self.ask(now);
        let live: HashSet<u64> = self.tracker.tracks.iter().map(|t| t.id).collect();
        self.painted.retain(|id, _| live.contains(id));
        let target = self.reader.target.clone();
        let mut quads = Vec::new();
        let mut order: Vec<&Track> = self.tracker.tracks.iter().filter(|t| t.settled()).collect();
        let priority = |t: &Track| {
            let to = geometry::sub(t.placed.centre(), gaze.0);
            let d = geometry::norm(to).max(0.01);
            let off = 1.0 - geometry::dot(geometry::scale(to, 1.0 / d), gaze.1);
            off * 4.0 + d * 0.05
        };
        order.sort_by(|a, b| priority(a).total_cmp(&priority(b)));
        let mut wanted = Vec::new();
        for t in order {
            if wanted.len() >= MOST_SHOWN {
                break;
            }
            let Some(translation) = self.cache.get(&target, &t.key) else { continue };
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
        Scene { quads, activity, timings, lines: self.tracker.tracks.len(), waiting_s: self.asked_at.map(|t| t.elapsed().as_secs_f32()) }
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
            let (request, answer, took) = match message {
                FromTranslator::Warm(took) => {
                    self.warm = true;
                    self.log.push(format!("translator ready after {:.0} ms of warming up", took.as_secs_f64() * 1000.0)); // i18n: not shown (a log's line)
                    continue;
                }
                FromTranslator::Answer(r, a, took) => (r, a, took),
            };
            self.answered += 1;
            self.asked_at = None;
            self.log.push(format!(
                "batch of {} lines: {} in {:.0} ms ({:.0} ms queued behind the translator)", // i18n: not shown (a log's line)
                request.keys.len(),
                if answer.is_ok() { "translated" } else { "failed" }, // i18n: not shown (a log's line)
                took.as_secs_f64() * 1000.0,
                request.asked.elapsed().saturating_sub(took).as_secs_f64() * 1000.0
            ));
            for k in &request.keys {
                self.in_flight.remove(&(request.target.clone(), k.clone()));
            }
            match answer {
                Ok(translations) => {
                    self.last_failure = None;
                    if self.log_text {
                        self.log.push(format!("into {}: {:?}", request.target, request.keys.iter().zip(&translations).collect::<Vec<_>>())); // i18n: not shown (a log's line)
                    }
                    for (k, t) in request.keys.iter().zip(translations) {
                        self.cache.put(&request.target, k, t);
                    }
                }
                Err(f) => {
                    for k in &request.keys {
                        self.failed.insert((request.target.clone(), k.clone()), now);
                    }
                    self.last_failure = Some(f);
                }
            }
        }
    }

    fn ask(&mut self, now: f64) {
        let target = self.reader.target.clone();
        // One request at a time: the lines of the next look join the next batch.
        if self.in_flight.iter().any(|(t, _)| *t == target) {
            return;
        }
        let mut keys: Vec<String> = Vec::new();
        for t in &self.tracker.tracks {
            let k = (target.clone(), t.key.clone());
            if self.cache.get(&target, &t.key).is_some() || keys.contains(&t.key) {
                continue;
            }
            if self.failed.get(&k).is_some_and(|at| now - at < 8.0) {
                continue;
            }
            keys.push(t.key.clone());
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

/// A translation equal to its source (ignoring case, spaces and punctuation) is not drawn.
pub fn same_text(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect::<String>();
    norm(a) == norm(b)
}

fn colours_far(a: Colours, b: Colours) -> bool {
    let d = |x: [u8; 3], y: [u8; 3]| (0..3).map(|i| (x[i] as i32 - y[i] as i32).abs()).max().unwrap_or(0);
    d(a.background, b.background) > 24 || d(a.text, b.text) > 40
}
