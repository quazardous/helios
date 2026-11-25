//! What the desktop's host does that the tests do in its place: it hears
//! the workspace's network changed (`darkroom.network`, on
//! `workspace.applied`) and passes the image shown through it.

use std::cell::Cell;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use helios_gui::Shell;

thread_local! {
    static CHANGED: Cell<usize> = const { Cell::new(0) };
}

/// Listens for the workspace's network changed on `shell`'s bus, as the
/// host does: counted, for [`network_changed`].
pub fn listen_network(shell: &Shell) {
    shell
        .listen_network(|| CHANGED.with(|c| c.set(c.get() + 1)))
        .unwrap();
}

/// How many times the network changed since the last call.
pub fn network_changed() -> usize {
    CHANGED.with(|c| c.replace(0))
}

/// The frames a step takes, then what the host does when the network
/// changed: the image kept open through it. A panel busy with work on a
/// thread of its own asks frames until the work lands, in no fixed
/// number (more on a slower machine): a run that does not settle within
/// the harness's steps is run again, within a minute, not failed.
pub fn follow(h: &mut Harness<'static, Shell>) {
    for _ in 0..3 {
        settle(h);
    }
    if network_changed() > 0 {
        assert!(h.state_mut().keep_image_open().unwrap(), "kept in place");
    }
    for _ in 0..2 {
        settle(h);
    }
}

/// Frames until a run settles, within a minute: a deadline, not a guess
/// at how long the work takes.
fn settle(h: &mut Harness<'static, Shell>) {
    h.settle();
}

/// [`Harness::run`] for a window whose panels may be busy with work on
/// threads of their own (the preview, the scopes, the navigator): a run
/// that does not settle within the harness's steps (more frames on a
/// slower machine, a software adapter) is run again, within a minute,
/// not failed. A run that settles is the same as `run`.
pub trait Settle {
    fn settle(&mut self);
}

impl<S> Settle for Harness<'_, S> {
    fn settle(&mut self) {
        let end = Instant::now() + Duration::from_mins(1);
        while self.run_ok().is_none() {
            assert!(Instant::now() < end, "the window settles");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
