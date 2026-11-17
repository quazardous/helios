//! Timed spans of the work on the photo-open path, for whoever listens.
//!
//! A span ([`span`]) times a piece of work from its start until it is
//! dropped, and hands a [`Record`] (its name, a detail, when it started
//! and how long it took, the thread it ran on) to every listener
//! ([`listen`]). The engine marks each step it computes
//! (`engine.step`, `engine.reduce`, `engine.source`), modules mark their
//! own heavy calls (`demosaic`, with its algorithm and megapixels), the
//! desktop marks the open itself (`photo.open`, `photo.decode`,
//! `photo.graph`, `viewport.tiles`, `preview.render`, `scopes.histogram`).
//!
//! With no listener a span is one relaxed atomic load: its detail is not
//! built, no clock is read. The open-timing bench and the tests listen;
//! the desktop listens when `HELIOS_OPEN_TRACE` is set.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

/// A span that ended.
#[derive(Clone, Debug)]
pub struct Record {
    /// What was timed: `engine.step`, `demosaic`, `photo.decode`...
    pub name: &'static str,
    /// What it worked on (a module's name, an algorithm, megapixels).
    pub detail: String,
    /// When it started.
    pub start: Instant,
    /// How long it took.
    pub elapsed: Duration,
    /// The name of the thread it ran on, if it has one.
    pub thread: Option<String>,
}

impl Record {
    /// When it ended.
    pub fn end(&self) -> Instant {
        self.start + self.elapsed
    }

    /// How long it took, in milliseconds.
    pub fn ms(&self) -> f64 {
        self.elapsed.as_secs_f64() * 1e3
    }
}

type Sink = Arc<dyn Fn(&Record) + Send + Sync>;

/// How many listeners there are: zero keeps spans free.
static LISTENERS: AtomicUsize = AtomicUsize::new(0);
static SINKS: RwLock<Vec<(u64, Sink)>> = RwLock::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Whether anyone listens.
#[inline]
pub fn listening() -> bool {
    LISTENERS.load(Ordering::Relaxed) > 0
}

/// A span being timed; it ends when dropped.
#[must_use = "a span ends when it is dropped"]
pub struct Span(Option<(&'static str, String, Instant)>);

/// Starts a span named `name`; `detail` is only called when someone
/// listens.
#[inline]
pub fn span(name: &'static str, detail: impl FnOnce() -> String) -> Span {
    if !listening() {
        return Span(None);
    }
    Span(Some((name, detail(), Instant::now())))
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some((name, detail, start)) = self.0.take() else {
            return;
        };
        let record = Record {
            name,
            detail,
            start,
            elapsed: start.elapsed(),
            thread: std::thread::current().name().map(String::from),
        };
        let sinks = SINKS.read().unwrap_or_else(PoisonError::into_inner);
        for (_, sink) in sinks.iter() {
            sink(&record);
        }
    }
}

/// A listener: it hears spans until dropped.
#[must_use = "the listener stops when dropped"]
pub struct Listener(u64);

/// Calls `sink` with every span that ends, until the listener is dropped.
pub fn listen(sink: impl Fn(&Record) + Send + Sync + 'static) -> Listener {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    SINKS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .push((id, Arc::new(sink)));
    LISTENERS.fetch_add(1, Ordering::Relaxed);
    Listener(id)
}

impl Drop for Listener {
    fn drop(&mut self) {
        SINKS
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(id, _)| *id != self.0);
        LISTENERS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Every span that ends, kept in order, until the listener is dropped.
pub fn collect() -> (Listener, Arc<Mutex<Vec<Record>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let kept = records.clone();
    let listener = listen(move |r| {
        kept.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(r.clone());
    });
    (listener, records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_reaches_listeners_only_while_they_listen() {
        let built = Arc::new(AtomicUsize::new(0));
        let (listener, records) = collect();
        {
            let b = built.clone();
            let _s = span("trace.test.one", move || {
                b.fetch_add(1, Ordering::Relaxed);
                "detail".into()
            });
        }
        drop(listener);
        let mine: Vec<_> = records
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.name == "trace.test.one")
            .cloned()
            .collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].detail, "detail");
        assert_eq!(built.load(Ordering::Relaxed), 1);
        // Nothing listening (unless a test alongside does): the record
        // is not kept by the dropped listener.
        drop(span("trace.test.two", String::new));
        assert!(
            records
                .lock()
                .unwrap()
                .iter()
                .all(|r| r.name != "trace.test.two")
        );
    }
}
