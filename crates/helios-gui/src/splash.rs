//! The boot and end splashes: one screen for the start and the end of
//! Helios, like a system's boot and poweroff. The Helios mark, the
//! version, a spinner, and one line per step as it runs, ticked when
//! done.
//!
//! The steps are not the splash's: the parts that run them declare them
//! on their phase (`helios_node_core::phases`: the node opening and
//! closing the library, the window saving the changes…), and the splash
//! draws whatever the phase holds. A step that lasts more than [`STUCK`]
//! says why (what its part said, else that it is still working) and, at
//! the end, offers "Quit now" when its part said stopping early is safe.
//!
//! Both are drawn in the main window: the boot splash from its first
//! frame until the start's steps are done ([`Boot`]), the end splash from
//! the frame that sees the quit until its steps are ([`End`]). The window
//! keeps drawing all along: the work off the interface's thread runs on
//! its own, the work on it runs one step per frame.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use helios_core::Message;
use helios_node_core::phases::{Phase, Running};

/// How long a step runs before the splash says why it lasts.
pub const STUCK: Duration = Duration::from_secs(3);

/// How often the splash draws while steps run (its spinner, the steps
/// done off the interface's thread).
const REDRAW: Duration = Duration::from_millis(30);

/// Which splash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Boot,
    End,
}

/// The mark a done step shows (its accessible label too).
pub const DONE: &str = "✔";

fn title(kind: Kind) -> Message {
    match kind {
        Kind::Boot => Message::new("gui-splash-starting", "Starting Helios…"),
        Kind::End => Message::new("gui-splash-closing", "Closing Helios…"),
    }
}

/// Draws the splash over the whole of `ui`: `phase`'s steps as they
/// stand at `now`, a step past `stuck_after` with why it lasts. "Quit
/// now", at the end, asks the phase to stop waiting.
pub fn show(ui: &mut egui::Ui, kind: Kind, phase: &Phase, now: Instant, stuck_after: Duration) {
    let ctx = ui.ctx().clone();
    let text = |m: Message| crate::i18n::text(&ctx, m);
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
    let steps = phase.steps();
    let width = 420.0_f32.min(rect.width());
    let mut column = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_center_size(
                rect.center(),
                egui::vec2(width, rect.height()),
            ))
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    let ui = &mut column;
    let lines = steps.len() as f32;
    ui.add_space(((rect.height() - 220.0 - lines * 24.0) / 2.0).max(8.0));
    let logo = crate::plugins::logo(ui.ctx());
    ui.add(egui::Image::new((logo.id(), egui::vec2(64.0, 64.0))));
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Helios").heading().strong());
    ui.label(egui::RichText::new(env!("HELIOS_VERSION")).weak().small());
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let title = text(title(kind));
        let wide = ui.spacing().interact_size.y + ui.spacing().item_spacing.x;
        let galley_width = ui.fonts_mut(|f| {
            f.layout_no_wrap(
                title.clone(),
                egui::TextStyle::Body.resolve(ui.style()),
                egui::Color32::WHITE,
            )
            .size()
            .x
        });
        ui.add_space(((ui.available_width() - galley_width - wide) / 2.0).max(0.0));
        ui.add(egui::Spinner::new());
        ui.label(title);
    });
    ui.add_space(12.0);
    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
        for step in &steps {
            ui.horizontal(|ui| {
                let icon = ui.spacing().interact_size.y;
                ui.allocate_ui(egui::vec2(icon, icon), |ui| {
                    if step.done() {
                        ui.label(egui::RichText::new(DONE).color(ui.visuals().selection.bg_fill));
                    } else if step.running() {
                        ui.add(egui::Spinner::new().size(icon * 0.7));
                    } else {
                        ui.label(egui::RichText::new("·").weak());
                    }
                });
                let label = egui::RichText::new(text(step.label.clone()));
                ui.label(if step.running() {
                    label.strong()
                } else {
                    label
                });
            });
            if step.running() && step.running_for(now) >= stuck_after {
                ui.horizontal(|ui| {
                    ui.add_space(ui.spacing().interact_size.y + ui.spacing().item_spacing.x);
                    let why = step.why.clone().unwrap_or_else(|| {
                        Message::new("gui-splash-still-working", "Still working…")
                    });
                    ui.label(egui::RichText::new(text(why)).weak());
                    if kind == Kind::End
                        && step.can_skip
                        && ui
                            .button(text(Message::new("gui-splash-quit-now", "Quit now")))
                            .clicked()
                    {
                        phase.quit_now();
                    }
                });
            }
        }
    });
    if phase.busy() {
        ui.ctx().request_repaint_after(REDRAW);
    }
}

/// The boot splash in the main window: drawn until the start's steps
/// are done, once at least (a fast start shows it for one frame, no
/// longer).
#[derive(Debug)]
pub struct Boot {
    phase: Phase,
    drawn: bool,
    over: bool,
}

impl Boot {
    pub fn new(phase: Phase) -> Self {
        Boot {
            phase,
            drawn: false,
            over: false,
        }
    }

    /// Whether the start is over: the shell is drawn from now on.
    pub fn over(&self) -> bool {
        self.over
    }

    /// Draws the splash while the start goes on: whether it is over (this
    /// pass draws nothing then, the shell takes the window).
    pub fn pass(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.over && self.drawn && !self.phase.busy() {
            self.over = true;
            crate::start::mark("boot splash over");
            ui.ctx().request_repaint();
        }
        if self.over {
            return true;
        }
        show(ui, Kind::Boot, &self.phase, Instant::now(), STUCK);
        self.drawn = true;
        // Once more at least: the splash gives way when the steps are done.
        ui.ctx().request_repaint_after(REDRAW);
        false
    }
}

/// Where the end stands, after a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// Steps still run: the window stays, drawing the splash.
    Running,
    /// Every step is done: the window may close.
    Done,
    /// The person asked to quit now: the window closes, the steps left
    /// are left.
    QuitNow,
}

/// A step the end runs on the interface's thread, with the host's state.
type Work<T> = Box<dyn FnOnce(&mut T)>;

/// The end splash in the main window, and the end's steps the host runs
/// on the interface's thread (one per frame, the splash drawn between
/// them), then the work going on off it (`waiting_for`).
pub struct End<T> {
    phase: Phase,
    steps: VecDeque<(Running, Work<T>)>,
    waits: Vec<Box<dyn Fn() -> bool>>,
    stuck_after: Duration,
    give_up: Option<Duration>,
    began: Option<Instant>,
    drawn: bool,
    outcome: Ending,
}

impl<T> End<T> {
    pub fn new(phase: Phase) -> Self {
        End {
            phase,
            steps: VecDeque::new(),
            waits: Vec::new(),
            stuck_after: STUCK,
            give_up: None,
            began: None,
            drawn: false,
            outcome: Ending::Running,
        }
    }

    /// A step run on the interface's thread, listed from now on.
    #[must_use]
    pub fn then(mut self, label: Message, work: impl FnOnce(&mut T) + 'static) -> Self {
        let step = self.phase.declare(label);
        self.steps.push_back((step, Box::new(work)));
        self
    }

    /// Work going on off the interface's thread (it declares its own
    /// steps): the end waits until `done` says it is.
    #[must_use]
    pub fn waiting_for(mut self, done: impl Fn() -> bool + 'static) -> Self {
        self.waits.push(Box::new(done));
        self
    }

    /// How long a step runs before the splash says why ([`STUCK`]).
    #[must_use]
    pub fn stuck_after(mut self, after: Duration) -> Self {
        self.stuck_after = after;
        self
    }

    /// Past `limit` from its first pass, the end goes on as if "Quit now"
    /// was chosen: a step that never ends does not hold the window.
    #[must_use]
    pub fn give_up_after(mut self, limit: Duration) -> Self {
        self.give_up = Some(limit);
        self
    }

    /// Where the end stands.
    pub fn outcome(&self) -> Ending {
        self.outcome
    }

    /// One pass: the next step on the interface's thread once the splash
    /// was drawn, the splash, then where the end stands.
    pub fn pass(&mut self, ui: &mut egui::Ui, state: &mut T) -> Ending {
        if self.outcome != Ending::Running {
            // The last frames before the window goes: the splash still.
            show(ui, Kind::End, &self.phase, Instant::now(), self.stuck_after);
            return self.outcome;
        }
        let began = *self.began.get_or_insert_with(Instant::now);
        if self.drawn
            && let Some((step, work)) = self.steps.pop_front()
        {
            step.start();
            work(state);
            drop(step);
        }
        show(ui, Kind::End, &self.phase, Instant::now(), self.stuck_after);
        self.drawn = true;
        let too_long = self.give_up.is_some_and(|limit| began.elapsed() >= limit);
        if self.phase.quitting_now() || too_long {
            if too_long {
                log::warn!(target: "helios::quit", "the end's steps passed their limit: left");
            }
            self.outcome = Ending::QuitNow;
        } else if self.steps.is_empty() && self.waits.iter().all(|w| w()) && !self.phase.busy() {
            self.outcome = Ending::Done;
        } else {
            ui.ctx().request_repaint_after(REDRAW);
        }
        self.outcome
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn opening() -> Message {
        Message::new("test-opening", "Opening the library")
    }

    fn reading() -> Message {
        Message::new("test-reading", "Reading the library")
    }

    /// The boot splash lists the steps declared, ticks them as they end,
    /// and gives way once they are all done.
    #[test]
    fn the_boot_splash_lists_and_ticks_its_steps_then_gives_way() {
        let phase = Phase::new();
        let open = phase.begin(opening());
        let read = phase.declare(reading());
        let mut h = Harness::builder()
            .with_size(egui::vec2(640.0, 400.0))
            .build_ui_state(
                |ui, (boot, over): &mut (Boot, bool)| {
                    *over = boot.pass(ui);
                },
                (Boot::new(phase.clone()), false),
            );
        h.run_steps(2);
        h.get_by_label("Starting Helios…");
        h.get_by_label("Opening the library");
        h.get_by_label("Reading the library");
        assert_eq!(h.query_all_by_label(DONE).count(), 0);
        open.done();
        read.start();
        h.run_steps(2);
        assert_eq!(h.query_all_by_label(DONE).count(), 1, "one ticked");
        assert!(!h.state().1, "still booting");
        read.done();
        h.run_steps(2);
        assert!(h.state().1, "the start is over: the shell takes the window");
        assert!(h.query_by_label("Starting Helios…").is_none());
    }

    #[derive(Default)]
    struct Host {
        saved: bool,
        frames: u32,
        outcome: Option<Ending>,
    }

    fn end_harness(end: End<Host>) -> Harness<'static, (End<Host>, Host)> {
        Harness::builder()
            .with_size(egui::vec2(640.0, 400.0))
            .build_ui_state(
                |ui, (end, host): &mut (End<Host>, Host)| {
                    host.frames += 1;
                    let outcome = end.pass(ui, host);
                    host.outcome = Some(outcome);
                },
                (end, Host::default()),
            )
    }

    /// The end splash shows from the first pass, runs the window's steps
    /// one per frame, keeps drawing while the work off the thread goes on,
    /// and is done once it ends.
    #[test]
    fn the_end_splash_runs_its_steps_and_keeps_drawing_until_they_end() {
        let phase = Phase::new();
        let off_thread = Arc::new(AtomicBool::new(false));
        let ended = off_thread.clone();
        let end = End::new(phase.clone())
            .then(
                Message::new("test-saving", "Saving your changes"),
                |h: &mut Host| {
                    h.saved = true;
                },
            )
            .waiting_for(move || ended.load(Ordering::SeqCst));
        let mut h = end_harness(end);
        h.run_steps(1);
        h.get_by_label("Closing Helios…");
        h.get_by_label("Saving your changes");
        assert!(!h.state().1.saved, "the splash drawn before the step runs");
        h.run_steps(1);
        assert!(h.state().1.saved);
        // The node's own step, declared off the thread.
        let closing = phase.begin(Message::new(
            "test-closing",
            "Saving and closing the library",
        ));
        h.run_steps(5);
        h.get_by_label("Saving and closing the library");
        assert_eq!(h.state().1.outcome, Some(Ending::Running));
        assert!(h.state().1.frames >= 7, "frames keep coming");
        drop(closing);
        off_thread.store(true, Ordering::SeqCst);
        h.run_steps(1);
        assert_eq!(h.state().1.outcome, Some(Ending::Done));
        assert_eq!(h.query_all_by_label(DONE).count(), 2);
    }

    /// A step that lasts says why; "Quit now" ends the wait when its part
    /// said stopping early is safe.
    #[test]
    fn a_stuck_step_says_why_and_quit_now_ends_it() {
        let phase = Phase::new();
        let end = End::new(phase.clone()).stuck_after(Duration::ZERO);
        let jobs = phase
            .begin(Message::new("test-jobs", "Stopping background jobs"))
            .can_skip();
        jobs.why(Message::new(
            "test-why",
            "Waiting for 2 running jobs to finish",
        ));
        let mut h = end_harness(end);
        h.run_steps(2);
        h.get_by_label("Waiting for 2 running jobs to finish");
        assert_eq!(h.state().1.outcome, Some(Ending::Running));
        h.get_by_label("Quit now").click();
        h.run_steps(1);
        assert_eq!(h.state().1.outcome, Some(Ending::QuitNow));
        drop(jobs);
    }

    /// A step that cannot be left says it is still working, without
    /// "Quit now".
    #[test]
    fn a_step_that_cannot_be_left_offers_no_quit_now() {
        let phase = Phase::new();
        let _saving = phase.begin(Message::new("test-saving", "Saving your changes"));
        let mut h = Harness::builder()
            .with_size(egui::vec2(640.0, 400.0))
            .build_ui(move |ui| {
                show(ui, Kind::End, &phase, Instant::now(), Duration::ZERO);
            });
        h.run_steps(2);
        h.get_by_label("Still working…");
        assert!(h.query_by_label("Quit now").is_none());
    }
}
