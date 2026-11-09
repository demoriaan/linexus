//! Profile pictures, fetched on their own thread: a slow picture server must
//! not hold up voice events or the buttons' commands.

use nexus_render::tiny_skia::Pixmap;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

/// Avatars are drawn at up to 30 px; twice that keeps them sharp.
const AVATAR_PIXELS: u32 = 60;
/// Pictures kept in memory. Past this they are forgotten and read again from
/// the art cache on disk.
const MAX_KNOWN: usize = 256;
/// Pictures waiting for the thread; the rest are asked for again later.
const MAX_WAITING: usize = 32;

type Fetch = Box<dyn Fn(&str) -> Option<Pixmap> + Send>;

pub(super) struct Images {
    /// Decoded pictures by URL; None when one could not be fetched.
    known: HashMap<String, Option<Arc<Pixmap>>>,
    waiting: HashSet<String>,
    /// Dropping this ends the thread.
    jobs: Sender<String>,
    answers: Receiver<(String, Option<Pixmap>)>,
}

impl Images {
    pub(super) fn start() -> Self {
        Self::with_fetch(Box::new(|url| {
            let bytes = nexus_sensors::fetch_art_bytes(url)?;
            nexus_render::icons::decode_image(&bytes, AVATAR_PIXELS)
        }))
    }

    pub(super) fn with_fetch(fetch: Fetch) -> Self {
        let (jobs, requested) = mpsc::channel::<String>();
        let (answer, answers) = mpsc::channel();
        // Without the thread nothing is ever answered; people then show
        // without pictures.
        let _ = thread::Builder::new()
            .name("discord-images".into())
            .spawn(move || {
                for url in requested {
                    // A picture that trips a decoder is skipped, not the
                    // ones after it.
                    let image = catch_unwind(AssertUnwindSafe(|| fetch(&url)))
                        .ok()
                        .flatten();
                    if answer.send((url, image)).is_err() {
                        return;
                    }
                }
            });
        Self {
            known: HashMap::new(),
            waiting: HashSet::new(),
            jobs,
            answers,
        }
    }

    pub(super) fn get(&self, url: &str) -> Option<Arc<Pixmap>> {
        self.known.get(url).cloned().flatten()
    }

    /// Whether the answer for `url` is in, picture or not.
    pub(super) fn answered(&self, url: &str) -> bool {
        self.known.contains_key(url)
    }

    /// Asks for a picture, unless it is known or on its way.
    pub(super) fn ask(&mut self, url: &str) {
        if self.answered(url) || self.waiting.contains(url) || self.waiting.len() >= MAX_WAITING {
            return;
        }
        if self.jobs.send(url.to_string()).is_ok() {
            self.waiting.insert(url.to_string());
        }
    }

    /// Takes in the pictures that arrived.
    pub(super) fn poll(&mut self) {
        while let Ok((url, image)) = self.answers.try_recv() {
            self.waiting.remove(&url);
            if self.known.len() >= MAX_KNOWN {
                self.known.clear();
            }
            self.known.insert(url, image.map(Arc::new));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn settled(images: &mut Images, url: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !images.answered(url) && Instant::now() < deadline {
            images.poll();
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn pictures_arrive_without_blocking_and_failures_are_remembered() {
        let mut images = Images::with_fetch(Box::new(|url| {
            (url == "good").then(|| Pixmap::new(2, 2).unwrap())
        }));
        images.ask("good");
        images.ask("good");
        images.ask("bad");
        settled(&mut images, "good");
        settled(&mut images, "bad");
        assert!(images.get("good").is_some());
        assert!(images.get("bad").is_none());
        assert!(images.answered("bad"));
    }

    #[test]
    fn a_decoder_that_panics_does_not_end_the_thread() {
        let mut images = Images::with_fetch(Box::new(|url| {
            assert_ne!(url, "boom");
            Some(Pixmap::new(1, 1).unwrap())
        }));
        images.ask("boom");
        images.ask("after");
        settled(&mut images, "after");
        assert!(images.get("after").is_some());
    }

    #[test]
    fn the_number_of_pictures_kept_is_bounded() {
        let mut images = Images::with_fetch(Box::new(|_| Pixmap::new(1, 1)));
        for n in 0..MAX_KNOWN * 2 {
            let url = n.to_string();
            images.ask(&url);
            settled(&mut images, &url);
            assert!(images.known.len() <= MAX_KNOWN);
        }
    }
}
