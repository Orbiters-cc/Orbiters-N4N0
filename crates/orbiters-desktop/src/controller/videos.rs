//! The videos the worlds' players played (`orbiters_core::video_history`), kept by the tray service from VRChat's logs
//! beside the visited worlds: the newest log as the follower reads it, the older ones (and the newest up to the
//! follower) on the watchers' ten-second beat. YouTube's titles and channels are asked for a few at a time on a thread
//! of their own, as a browser would, never in a burst. The window's Videos page reads the file.

use orbiters_core::video_history::History;
use orbiters_core::vrchat_log::LogEvent;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Titles asked for in one go, and the pause between two.
const TITLES_AT_ONCE: usize = 12;
const TITLE_PAUSE: Duration = Duration::from_millis(800);
/// YouTube could not be asked (no network): not again for this long.
const OFFLINE_PAUSE: Duration = Duration::from_secs(120);

/// What YouTube said of a link: its title and channel, none (private, removed), or that it could not be asked.
type Answer = (String, Result<Option<(String, String)>, String>);

pub(super) struct Videos {
    file: PathBuf,
    history: History,
    changed: bool,
    /// The titles being asked for.
    titles: Option<mpsc::Receiver<Answer>>,
    offline_until: Option<Instant>,
}

impl Videos {
    /// The videos kept in `video-history.json`.
    pub(super) fn load() -> Self {
        let file = orbiters_core::video_history::history_file();
        Self { history: History::load(&file), file, changed: false, titles: None, offline_until: None }
    }

    /// The follower's last read (`last_read`: the log's name and the bytes read, `events` what they said); on `beat`, the
    /// logs it did not read, the titles to ask for, and the file saved when anything changed.
    pub(super) fn step(&mut self, last_read: Option<(&str, u64, u64)>, events: &[LogEvent], beat: bool) {
        if let Some((name, from, to)) = last_read {
            self.changed |= self.history.follow(name, from, to, events);
        }
        self.take_titles();
        if !beat {
            return;
        }
        if let Some(dir) = orbiters_core::paths::vrchat_log_dir() {
            self.changed |= self.history.read_logs_following(&dir, last_read.map(|(name, _, to)| (name, to)));
        }
        self.ask_titles();
        if std::mem::take(&mut self.changed) {
            if let Err(e) = self.history.save(&self.file) {
                eprintln!("video history: {e}");
            }
        }
    }

    /// What YouTube answered so far.
    fn take_titles(&mut self) {
        let Some(rx) = &self.titles else { return };
        loop {
            match rx.try_recv() {
                Ok((url, Ok(found))) => self.changed |= self.history.set_title(&url, found),
                Ok((_, Err(e))) => {
                    eprintln!("video history: YouTube's titles: {e}");
                    self.offline_until = Some(Instant::now() + OFFLINE_PAUSE);
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.titles = None;
                    return;
                }
            }
        }
    }

    /// Asks YouTube for the titles of the latest videos without one, unless it is being asked or could not be.
    fn ask_titles(&mut self) {
        if self.titles.is_some() || self.offline_until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let urls = self.history.wanting_titles(TITLES_AT_ONCE);
        if urls.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("video-titles".into()).spawn(move || ask(urls, &tx, orbiters_n4n0_core::videos::youtube_oembed));
        if spawned.is_ok() {
            self.titles = Some(rx);
        }
    }
}

/// Asks `oembed` for each of `urls` in turn, a pause between two; stops at the first that could not be asked.
fn ask(urls: Vec<String>, tx: &mpsc::Sender<Answer>, oembed: impl Fn(&str) -> anyhow::Result<Option<(String, String)>>) {
    for (i, url) in urls.into_iter().enumerate() {
        if i > 0 {
            std::thread::sleep(TITLE_PAUSE);
        }
        let answer = oembed(&url).map_err(|e| format!("{e:#}"));
        let failed = answer.is_err();
        if tx.send((url, answer)).is_err() || failed {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_asked_in_turn_until_youtube_cannot_be_reached() {
        let (tx, rx) = mpsc::channel();
        let urls = vec!["https://youtu.be/36YnV9STBqc".to_string(), "https://youtu.be/dQw4w9WgXcQ".to_string(), "https://youtu.be/aaaaaaaaaaa".to_string()];
        ask(urls, &tx, |url| match url {
            "https://youtu.be/36YnV9STBqc" => Ok(Some(("Lo-fi".into(), "Lofi Girl".into()))),
            "https://youtu.be/dQw4w9WgXcQ" => anyhow::bail!("offline"),
            _ => Ok(None),
        });
        drop(tx);
        let answers: Vec<Answer> = rx.iter().collect();
        assert_eq!(answers.len(), 2, "the third is not asked once YouTube could not be");
        assert_eq!(answers[0].1, Ok(Some(("Lo-fi".to_string(), "Lofi Girl".to_string()))));
        assert!(answers[1].1.is_err());
    }
}
