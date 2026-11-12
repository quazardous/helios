//! The stall watchdog: a frame of the interface's thread held longer
//! than a threshold leaves a trace, so a freeze nobody caught in the act
//! says where it was.
//!
//! The interface's thread marks each frame ([`frame`]) and, inside it,
//! the sections that may wait on something else ([`section`]): a lock, a
//! write to the node's vault, a window's pass. A thread of its own looks
//! at the marks a few times per threshold; when the frame has lasted
//! longer than the threshold, it logs the frame's sections at error level
//! and writes the same text to the crash folder (`stall-<time>.txt`),
//! once per stall; when the frame ends, how long it lasted is logged too.
//!
//! A thread cannot take another thread's backtrace, so the marks are the
//! trace: the sections open, outermost first, each with how long it has
//! been open. Debug builds add the sections the frame closed before the
//! stall, with their lengths, to tell a slow frame from a stuck one.
//!
//! The marks cost a lock and a clock read each; they are taken a few
//! times per frame.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a frame may hold the interface's thread before it is
/// reported.
pub const THRESHOLD: Duration = Duration::from_secs(2);

/// How many sections closed before the stall debug builds keep.
const CLOSED_KEPT: usize = 16;

/// The marks of one thread: the frame going and its sections.
#[derive(Default)]
pub struct Marks {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// When the frame going began; none between frames.
    frame: Option<(&'static str, Instant)>,
    /// The sections open, outermost first.
    open: Vec<(&'static str, Instant)>,
    /// The sections the frame closed, last first (debug builds).
    closed: Vec<(&'static str, Duration)>,
    /// The frame going was reported.
    reported: bool,
    /// Frames counted, to tell one stall from the next.
    frames: u64,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Marks {
    /// Marks a frame begun; it ends when the guard is dropped. A frame
    /// begun inside another (a window's pass run within the main
    /// window's) is a section of it.
    pub fn frame(self: &Arc<Self>, name: &'static str) -> Frame {
        let mut s = lock(&self.state);
        if s.frame.is_some() {
            s.open.push((name, Instant::now()));
            return Frame {
                marks: self.clone(),
                inner: Some(name),
            };
        }
        s.frame = Some((name, Instant::now()));
        s.open.clear();
        s.closed.clear();
        s.reported = false;
        s.frames += 1;
        Frame {
            marks: self.clone(),
            inner: None,
        }
    }

    /// Marks a section open; it closes when the guard is dropped.
    pub fn section(self: &Arc<Self>, name: &'static str) -> Section {
        lock(&self.state).open.push((name, Instant::now()));
        Section {
            marks: self.clone(),
            name,
        }
    }

    /// What a stall report says when the frame going has lasted longer
    /// than `threshold` and was not reported yet; none otherwise. The
    /// frame is marked reported.
    pub fn stalled(&self, threshold: Duration) -> Option<String> {
        let mut s = lock(&self.state);
        let (name, began) = s.frame?;
        let held = began.elapsed();
        if s.reported || held < threshold {
            return None;
        }
        s.reported = true;
        let now = Instant::now();
        let mut text = format!(
            "the interface's thread has been held {} ms in {name} (frame {})\n",
            held.as_millis(),
            s.frames
        );
        if s.open.is_empty() {
            text.push_str("no section open: the frame's own work, or egui's\n");
        }
        for (section, at) in &s.open {
            let _ = writeln!(
                text,
                "  in {section}, for {} ms",
                now.duration_since(*at).as_millis()
            );
        }
        if cfg!(debug_assertions) && !s.closed.is_empty() {
            text.push_str("closed before, last first:\n");
            for (section, took) in &s.closed {
                let _ = writeln!(text, "  {section}: {} ms", took.as_millis());
            }
        }
        Some(text)
    }
}

/// A frame going: ended when dropped.
pub struct Frame {
    marks: Arc<Marks>,
    /// Begun inside another frame: a section of it.
    inner: Option<&'static str>,
}

impl Drop for Frame {
    fn drop(&mut self) {
        if let Some(name) = self.inner {
            close(&self.marks, name);
            return;
        }
        let mut s = lock(&self.marks.state);
        if let Some((name, began)) = s.frame.take()
            && s.reported
        {
            log::warn!(
                target: "helios::stall",
                "the interface's thread came back after {} ms in {name}",
                began.elapsed().as_millis()
            );
        }
        s.open.clear();
    }
}

/// A section open: closed when dropped.
pub struct Section {
    marks: Arc<Marks>,
    name: &'static str,
}

impl Drop for Section {
    fn drop(&mut self) {
        close(&self.marks, self.name);
    }
}

/// The section `name` closed: the innermost of that name.
fn close(marks: &Marks, name: &'static str) {
    let mut s = lock(&marks.state);
    if let Some(at) = s.open.iter().rposition(|(n, _)| *n == name) {
        let (_, began) = s.open.remove(at);
        if cfg!(debug_assertions) {
            s.closed.insert(0, (name, began.elapsed()));
            s.closed.truncate(CLOSED_KEPT);
        }
    }
}

/// The interface thread's marks.
fn global() -> &'static Arc<Marks> {
    static MARKS: OnceLock<Arc<Marks>> = OnceLock::new();
    MARKS.get_or_init(Arc::default)
}

/// Marks a frame of the interface's thread begun, ended with the guard.
pub fn frame(name: &'static str) -> Frame {
    global().frame(name)
}

/// Marks a section of the interface's frame open, closed with the guard.
/// Outside a frame (a test driving the shell) it costs the same and
/// reports nothing.
pub fn section(name: &'static str) -> Section {
    global().section(name)
}

/// The watchdog's thread, stopped when dropped (it is never joined: it
/// sees the stop at its next look).
pub struct Watchdog {
    stop: Arc<AtomicBool>,
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Watchdog {
    /// Watches the interface thread's marks ([`frame`], [`section`]),
    /// a stall reported to the log and to `dir` (the crash folder).
    pub fn start(dir: Option<PathBuf>) -> Option<Self> {
        Self::watch(global().clone(), THRESHOLD, dir)
    }

    /// Watches `marks`: a frame held longer than `threshold` reported to
    /// the log and to a file in `dir`.
    pub fn watch(marks: Arc<Marks>, threshold: Duration, dir: Option<PathBuf>) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let every = (threshold / 4).max(Duration::from_millis(10));
        let spawned = std::thread::Builder::new()
            .name("helios-stall-watchdog".into())
            .spawn(move || {
                while !stopped.load(Ordering::Relaxed) {
                    std::thread::sleep(every);
                    if let Some(text) = marks.stalled(threshold) {
                        report(&text, dir.as_deref());
                    }
                }
            });
        match spawned {
            Ok(_) => Some(Self { stop }),
            Err(e) => {
                log::warn!(target: "helios::stall", "no thread to watch for stalls: {e}");
                None
            }
        }
    }
}

/// A stall's text to the log and, with `dir`, to a file there.
fn report(text: &str, dir: Option<&std::path::Path>) {
    log::error!(target: "helios::stall", "{}", text.trim_end());
    let Some(dir) = dir else { return };
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let file = dir.join(format!("stall-{at}.txt"));
    let body = format!("Helios {} stalled\n{text}", env!("HELIOS_VERSION"));
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, body)) {
        log::warn!(target: "helios::stall", "{}: {e}", file.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame held past the threshold in a section: the watchdog writes
    /// one entry naming the section to the folder given, and nothing for
    /// a frame that ends in time.
    #[test]
    fn a_forced_stall_writes_its_section_to_the_crash_folder() {
        let dir = std::env::temp_dir().join(format!("helios-stall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let marks = Arc::new(Marks::default());
        let threshold = Duration::from_millis(100);
        let _watchdog = Watchdog::watch(marks.clone(), threshold, Some(dir.clone())).unwrap();
        // In time: nothing.
        {
            let _frame = marks.frame("main window");
            let _s = marks.section("quick");
        }
        std::thread::sleep(threshold * 3);
        assert!(!dir.exists(), "a frame in time was reported");
        // Held: one entry, naming the frame and the section.
        {
            let _frame = marks.frame("main window");
            let _outer = marks.section("import window");
            let _inner = marks.section("forced stall");
            std::thread::sleep(threshold * 5);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let entries = loop {
            let entries: Vec<_> = std::fs::read_dir(&dir)
                .map(|d| d.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            if !entries.is_empty() || Instant::now() > deadline {
                break entries;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(entries.len(), 1, "one entry per stall: {entries:?}");
        let text = std::fs::read_to_string(&entries[0]).unwrap();
        assert!(text.contains("main window"), "{text}");
        assert!(text.contains("in import window"), "{text}");
        assert!(text.contains("in forced stall"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The report's text is made once per frame, the sections open in
    /// order, the closed ones listed in debug builds.
    #[test]
    fn a_stall_is_reported_once_with_its_sections_in_order() {
        let marks = Arc::new(Marks::default());
        let _frame = marks.frame("main window");
        {
            let _done = marks.section("done before");
        }
        let _a = marks.section("outer");
        let _b = marks.section("inner");
        assert!(marks.stalled(Duration::from_mins(1)).is_none());
        let text = marks.stalled(Duration::ZERO).expect("reported");
        let (outer, inner) = (
            text.find("in outer").unwrap(),
            text.find("in inner").unwrap(),
        );
        assert!(outer < inner, "{text}");
        if cfg!(debug_assertions) {
            assert!(text.contains("done before"), "{text}");
        }
        assert!(marks.stalled(Duration::ZERO).is_none(), "reported twice");
    }
}
