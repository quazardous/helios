//! Shift and Ctrl on the viewport's handles, for every overlay that has
//! them: the masks' shapes and the plugins' tools alike.
//!
//! The viewport has one `before_handle` hook. Each frame, whoever draws
//! handles says, for each one, what Shift does to it (`egui_hud`'s
//! [`Snap`]: along the axes and diagonals, or an angle to 15° steps) and
//! the point an angle turns around; the hook then holds every drag to
//! its rule (`egui_hud::handle::snapped`), Ctrl (Cmd) moving any handle a
//! tenth of the pointer. A handle nobody spoke for gets Ctrl only.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use egui_pop::viewport::hooks::HandleMove;
use egui_pop::viewport::{Mapping, Verdict};
use helios_rack::handle::{self as hud, Snap};

/// How a handle is held by Shift, and the point an angle turns around.
pub type Constraint = (Option<Snap>, [f64; 2]);

/// The handles' rules this frame, by overlay id and handle index; one
/// shared by the masks and the plugins' tools, read by the hook.
#[derive(Clone, Debug, Default)]
pub struct Snaps(Rc<RefCell<BTreeMap<(String, u32), Constraint>>>);

impl Snaps {
    /// Forgets last frame's rules: called once before the handles are
    /// prepared.
    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    /// Handle `index` of overlay `overlay` snaps by `rule` around `pivot`.
    pub fn insert(&self, overlay: &str, index: u32, rule: Option<Snap>, pivot: [f64; 2]) {
        self.0
            .borrow_mut()
            .insert((overlay.to_owned(), index), (rule, pivot));
    }

    /// The rule of a handle, if one was said this frame.
    pub fn get(&self, overlay: &str, index: u32) -> Option<Constraint> {
        self.0.borrow().get(&(overlay.to_owned(), index)).copied()
    }

    /// Where a handle's move lands: Shift by its rule, Ctrl finely.
    pub fn snapped(&self, mv: &HandleMove) -> [f64; 2] {
        let (rule, pivot) = self.get(&mv.overlay, mv.handle).unwrap_or((None, mv.start));
        hud::snapped(
            rule,
            pivot,
            mv.start,
            mv.to,
            mv.modifiers.shift,
            mv.modifiers.command,
        )
    }

    /// The viewport's `before_handle` hook on these rules.
    #[allow(clippy::type_complexity)]
    pub fn hook(&self) -> Box<dyn FnMut(&HandleMove, &Mapping) -> Verdict<[f64; 2]>> {
        let snaps = self.clone();
        Box::new(move |mv: &HandleMove, _m: &Mapping| {
            let to = snaps.snapped(mv);
            if to == mv.to {
                Verdict::Allow
            } else {
                Verdict::Redirect(to)
            }
        })
    }
}
