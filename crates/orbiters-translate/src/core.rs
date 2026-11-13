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
}

struct Request {
    target: String,
    keys: Vec<String>,
    batch: Batch,
}

/// The translator on its own thread: requests in, answers out.
struct TranslatorThread {
    send: mpsc::Sender<Request>,
    answers: mpsc::Receiver<(Request, Result<Vec<String>, Failure>)>,
}

impl TranslatorThread {
    fn start(mut translator: Box<dyn Translator>) -> Self {
        let (send, requests) = mpsc::channel::<Request>();
        let (give, answers) = mpsc::channel();
        std::thread::Builder::new()
            .name("translate requests".into())
            .spawn(move || {
                for r in requests {
                    let answer = translator.translate(&r.batch);
                    if give.send((r, answer)).is_err() {
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
        }
    }

    /// Another translator (the person chose another provider): what was translated stays.
    pub fn set_translator(&mut self, translator: Box<dyn Translator>) {
        self.translator = TranslatorThread::start(translator);
        self.in_flight.clear();
        self.failed.clear();
        self.last_failure = None;
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
        let activity = match (&self.last_failure, waiting) {
            (Some(f), _) => Activity::Failed(f.clone()),
            (None, 0) => Activity::Looking,
            (None, n) => Activity::Translating(n),
        };
        Scene { quads, activity, timings, lines: self.tracker.tracks.len() }
    }

    fn collect(&mut self, now: f64) {
        while let Ok((request, answer)) = self.translator.answers.try_recv() {
            for k in &request.keys {
                self.in_flight.remove(&(request.target.clone(), k.clone()));
            }
            match answer {
                Ok(translations) => {
                    self.last_failure = None;
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
        if self.translator.send.send(Request { target, keys, batch }).is_err() {
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
