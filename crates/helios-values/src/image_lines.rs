//! Each image's history kept across switches: its line parked when
//! another image opens, found again when it opens again.
//!
//! The store holds one image's values at a time, so a switch takes the
//! image's line out ([`Store::park_image`]) before the next image's
//! values take the same paths. Back on the image, [`Store::resume_image`]
//! puts its line back once its values are loaded, so Ctrl+Z undoes its
//! last step as if it had never been left.
//!
//! A line comes back only onto the values it left: each path its steps
//! touched must hold what the line's head says (the last undo step's
//! value, the next redo step's value before it). An edit made elsewhere
//! meanwhile (an agent, a batch, another node) moves them, and the line
//! is dropped rather than undone into values it never made.
//!
//! Memory is bounded: [`PARKED_IMAGES`] images, the least recently left
//! dropped first, each with at most [`PARKED_STEPS`] undo steps, the
//! oldest dropped first. Lines are kept for the session only; saving
//! them with the image's versions is a later slice.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use helios_core::Value;

use crate::history_scope::{HistoryScope, Parked};
use crate::{Step, Store};

/// How many images' lines stay parked: the least recently left goes
/// first.
pub const PARKED_IMAGES: usize = 20;

/// The undo steps a parked line keeps: its oldest go first.
pub const PARKED_STEPS: usize = 500;

/// The parked image lines, the least recently left first.
pub(crate) type Images = VecDeque<(String, Parked)>;

impl Parked {
    fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty() && self.replaced.is_empty()
    }

    /// Drops its oldest undo steps past [`PARKED_STEPS`], and the
    /// replaced lines left from a step dropped.
    fn cap(&mut self) {
        let over = self.undo.len().saturating_sub(PARKED_STEPS);
        if over == 0 {
            return;
        }
        self.undo.drain(..over);
        let kept: BTreeSet<u64> = self.undo.iter().map(|s| s.id).collect();
        self.replaced.retain(|b| kept.contains(&b.after));
    }

    /// The value each path it touched holds at its head: after the last
    /// undo step, before the next redo step.
    fn head(&self) -> Vec<(&str, &Value)> {
        let mut undone: BTreeMap<&str, &Value> = BTreeMap::new();
        for step in &self.undo {
            for (path, _, after) in &step.changes {
                undone.insert(path, after);
            }
        }
        let mut redone: BTreeMap<&str, &Value> = BTreeMap::new();
        // The next redo is the last: the first change of each path in it
        // wins.
        for step in &self.redo {
            for (path, before, _) in step.changes.iter().rev() {
                redone.insert(path, before);
            }
        }
        undone.into_iter().chain(redone).collect()
    }
}

impl Store {
    /// The image `image` is left: its line (undo, redo and replaced) is
    /// parked under it, capped, the least recently left image's dropped
    /// past [`PARKED_IMAGES`]; then the image's history leaves as
    /// [`Store::leave_image`] says. An empty line parks nothing.
    pub fn park_image(&mut self, image: &str) {
        let mut line = self.in_history(HistoryScope::Image, |s| Parked {
            undo: std::mem::take(&mut s.undo),
            redo: std::mem::take(&mut s.redo),
            replaced: std::mem::take(&mut s.replaced),
        });
        self.images.retain(|(k, _)| k != image);
        if !line.is_empty() {
            line.cap();
            self.images.push_back((image.to_owned(), line));
            while self.images.len() > PARKED_IMAGES {
                self.images.pop_front();
            }
        }
        self.leave_image();
    }

    /// The image `image` opened again, its values loaded: its parked
    /// line comes back as the image's, when the values are still those
    /// it left and the image has no step of its own yet. Whether it came
    /// back; a line that does not fit is dropped.
    pub fn resume_image(&mut self, image: &str) -> bool {
        let Some(at) = self.images.iter().position(|(k, _)| k == image) else {
            return false;
        };
        let Some((_, line)) = self.images.remove(at) else {
            return false;
        };
        let fits = line
            .head()
            .into_iter()
            .all(|(path, v)| self.get(path).is_some_and(|e| &e.value == v));
        let fresh = self.in_history(HistoryScope::Image, |s| {
            s.undo.is_empty() && s.redo.is_empty()
        });
        if !(fits && fresh) {
            return false;
        }
        self.in_history(HistoryScope::Image, |s| {
            s.undo = line.undo;
            s.redo = line.redo;
            s.replaced = line.replaced;
        });
        self.history_rev += 1;
        self.history_moved(None);
        self.announce();
        true
    }

    /// The images whose line is parked, the least recently left first.
    pub fn parked_images(&self) -> impl Iterator<Item = &str> {
        self.images.iter().map(|(k, _)| k.as_str())
    }

    /// The undo steps parked for `image`, oldest first.
    pub fn parked_undo_steps(&self, image: &str) -> &[Step] {
        self.images
            .iter()
            .find(|(k, _)| k == image)
            .map_or(&[], |(_, p)| &p.undo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Change, Descriptor, Set};
    use helios_shell::commands::Origin;

    const EV: &str = "node/exposure/exposure";
    const STATE: &str = "workspace/state";

    fn store() -> Store {
        let mut s = Store::new();
        s.declare(EV, Descriptor::float("Exposure", -100.0, 2000.0, 0.0));
        s.declare(STATE, Descriptor::text("workspace", "a"));
        s
    }

    fn set(s: &mut Store, v: f64) {
        s.set(Set::new(EV, Change::To(Value::Float(v)), Origin::User))
            .unwrap();
    }

    fn ev(s: &Store) -> Value {
        s.get(EV).unwrap().value.clone()
    }

    /// What a switch does to the values: the next image's loaded.
    fn load(s: &mut Store, v: f64) {
        s.declare_with(
            EV,
            Descriptor::float("Exposure", -100.0, 2000.0, 0.0),
            Value::Float(v),
        );
    }

    #[test]
    fn a_to_b_to_a_undoes_a_s_last_step() {
        let mut s = store();
        set(&mut s, 1.0);
        set(&mut s, 2.0);
        s.park_image("a");
        assert!(s.undo_steps().is_empty(), "B starts with no step");
        load(&mut s, 0.0);
        set(&mut s, 5.0);
        s.park_image("b");
        load(&mut s, 2.0);
        assert!(s.resume_image("a"));
        assert_eq!(s.undo_steps().len(), 2);
        s.undo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(1.0), "A's last step undone");
        // B's line is still parked, A's left again with its redo.
        assert_eq!(s.parked_undo_steps("b").len(), 1);
        s.park_image("a");
        load(&mut s, 5.0);
        assert!(s.resume_image("b"));
        s.undo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(0.0));
        load(&mut s, 1.0);
        s.park_image("b");
        assert!(s.resume_image("a"));
        assert_eq!(s.redo_steps().len(), 1, "A's redo kept");
        s.redo(&Origin::User).unwrap();
        assert_eq!(ev(&s), Value::Float(2.0));
    }

    #[test]
    fn a_line_does_not_come_back_onto_values_changed_meanwhile() {
        let mut s = store();
        set(&mut s, 1.0);
        s.park_image("a");
        // An agent edited A elsewhere: its values are not those left.
        load(&mut s, 3.0);
        assert!(!s.resume_image("a"));
        assert!(s.undo_steps().is_empty());
        assert_eq!(s.parked_images().count(), 0, "dropped");
    }

    #[test]
    fn the_workspace_s_steps_stay_their_own() {
        let mut s = store();
        set(&mut s, 1.0);
        s.set(Set::new(
            STATE,
            Change::To(Value::Text("b".into())),
            Origin::User,
        ))
        .unwrap();
        s.park_image("a");
        assert_eq!(s.parked_undo_steps("a").len(), 1, "the exposure only");
        assert_eq!(s.undo_steps_of(HistoryScope::Workspace).len(), 1);
        assert!(s.resume_image("a"));
        assert_eq!(s.undo_steps().len(), 1);
    }

    #[test]
    fn memory_is_bounded_in_images_and_steps() {
        let mut s = store();
        for i in 1..=1000 {
            set(&mut s, f64::from(i));
        }
        s.park_image("first");
        assert_eq!(s.parked_undo_steps("first").len(), PARKED_STEPS);
        for i in 0..PARKED_IMAGES {
            set(&mut s, 2000.0 - i as f64);
            s.park_image(&format!("img{i}"));
        }
        assert_eq!(s.parked_images().count(), PARKED_IMAGES);
        assert!(
            s.parked_images().all(|k| k != "first"),
            "the least recently left went first"
        );
        // A capped line still comes back and undoes its newest steps.
        load(&mut s, 2000.0 - (PARKED_IMAGES - 1) as f64);
        let last = format!("img{}", PARKED_IMAGES - 1);
        assert!(s.resume_image(&last));
        assert_eq!(s.undo_steps().len(), 1);
    }

    #[test]
    fn an_empty_line_parks_nothing() {
        let mut s = store();
        s.park_image("a");
        assert_eq!(s.parked_images().count(), 0);
        assert!(!s.resume_image("a"));
    }
}
