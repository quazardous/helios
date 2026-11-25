//! Step markers in an image's versions, and the undo history rebuilt from
//! them (`docs/design/undo.md`, "The image's scope").
//!
//! Each version the desktop saves for a step says what kind of step it
//! was, in its metadata (not hashed, so ids do not change):
//! `{"step": {"kind": "do" | "undo" | "redo" | "progress" | "cancel"}}`.
//! From the line of versions and these markers, [`rebuild`] finds the
//! steps an undo walks back and those a redo makes again, so Ctrl+Z
//! after a restart (or a crash) undoes the same steps as before it.
//! Without the markers, an undo saved as a version would read as a new
//! step, and a rebuilt Ctrl+Z would undo the undo.
//!
//! A version with no marker (saved before markers, by an older node, an
//! import, an agent's tool) reads as `do`, except a label ending in
//! " (in progress)", which reads as `progress`. Nothing to migrate.

use serde_json::{Value, json};

use crate::versions::Version;

/// The metadata key that holds a version's step marker.
pub const STEP: &str = "step";

/// What kind of step a version was saved for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// A new step: a write, a gesture's end, a group, a jump.
    Do,
    /// One or more steps undone.
    Undo,
    /// One or more steps redone.
    Redo,
    /// Saved while a gesture goes on (a drag): not a step of its own.
    Progress,
    /// A gesture given up, its values put back: not a step either.
    Cancel,
}

impl StepKind {
    pub fn word(self) -> &'static str {
        match self {
            StepKind::Do => "do",
            StepKind::Undo => "undo",
            StepKind::Redo => "redo",
            StepKind::Progress => "progress",
            StepKind::Cancel => "cancel",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "do" => StepKind::Do,
            "undo" => StepKind::Undo,
            "redo" => StepKind::Redo,
            "progress" => StepKind::Progress,
            "cancel" => StepKind::Cancel,
            _ => return None,
        })
    }

    /// The kind `version` was saved for, read from its marker; a version
    /// with none (or one this build does not know) as said above.
    pub fn of(version: &Version) -> Self {
        let marked = version
            .meta
            .as_ref()
            .and_then(|m| m[STEP]["kind"].as_str())
            .and_then(Self::parse);
        marked.unwrap_or_else(|| {
            if version
                .label
                .as_deref()
                .is_some_and(|l| l.ends_with(" (in progress)"))
            {
                StepKind::Progress
            } else {
                StepKind::Do
            }
        })
    }
}

/// `meta` with the step marker `kind` added beside what it holds (the
/// workshop session); a `meta` that is not an object is kept under
/// nothing else and replaced.
pub fn marked(meta: Option<Value>, kind: StepKind) -> Value {
    let mut meta = match meta {
        Some(Value::Object(map)) => Value::Object(map),
        _ => json!({}),
    };
    meta[STEP] = json!({ "kind": kind.word() });
    meta
}

/// One step of a rebuilt history: the version that made it, and the
/// edit it started from (none: the image with no edit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RebuiltStep<'a> {
    pub before: Option<&'a Value>,
    pub version: &'a Version,
}

/// An image's undo history, rebuilt from its line of versions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rebuilt<'a> {
    /// Every step still reachable, oldest first.
    pub steps: Vec<RebuiltStep<'a>>,
    /// How many of the newest steps are undone: they are what a redo
    /// makes again, the oldest of them first. The others are what an
    /// undo walks back, the newest first.
    pub undone: usize,
}

impl<'a> Rebuilt<'a> {
    /// The steps an undo walks back, oldest first.
    pub fn undo(&self) -> &[RebuiltStep<'a>] {
        &self.steps[..self.steps.len() - self.undone]
    }

    /// The steps a redo makes again, the next redo first.
    pub fn redo(&self) -> &[RebuiltStep<'a>] {
        &self.steps[self.steps.len() - self.undone..]
    }
}

/// The undo history of `line` (an image's versions, oldest first, as
/// [`crate::Vault::history`] reads them).
///
/// The states the steps reached are kept in order, with a cursor on the
/// current one. A `do` drops the states past the cursor (an undone step
/// a new step replaces) and adds its own. An `undo` or a `redo` moves the
/// cursor to the nearest state with the version's edit, below or above
/// it; several steps undone between two saves are one move. An undo of
/// the first step reaches the unedited image, which has no version. A
/// replay that reaches no known state (a jump to a replaced line, an
/// undo of the workspace that moved the image's values) reads as a `do`:
/// undone, it goes back to where it started. `progress` and `cancel` versions are no step.
pub fn rebuild(line: &[Version]) -> Rebuilt<'_> {
    // The states, `None` the unedited image first.
    fn edit_of(s: Option<&Version>) -> Option<&Value> {
        s.map(|v| &v.edit)
    }
    let mut states: Vec<Option<&Version>> = vec![None];
    let mut at = 0usize;
    for version in line {
        let kind = StepKind::of(version);
        let replay = match kind {
            StepKind::Progress | StepKind::Cancel => continue,
            StepKind::Do => None,
            StepKind::Undo => {
                // An undo walks the steps below only. From the first
                // step, none with this edit: back to the unedited image,
                // which has no version to compare.
                let below = (1..at)
                    .rev()
                    .find(|&i| edit_of(states[i]) == Some(&version.edit));
                below.or((at == 1).then_some(0))
            }
            StepKind::Redo => {
                let above =
                    (at + 1..states.len()).find(|&i| edit_of(states[i]) == Some(&version.edit));
                above.or_else(|| {
                    (0..at)
                        .rev()
                        .find(|&i| edit_of(states[i]) == Some(&version.edit))
                })
            }
        };
        if let Some(i) = replay {
            at = i;
        } else {
            states.truncate(at + 1);
            states.push(Some(version));
            at = states.len() - 1;
        }
    }
    let steps = states
        .windows(2)
        .map(|w| RebuiltStep {
            before: edit_of(w[0]),
            version: w[1].expect("a step has its version"),
        })
        .collect::<Vec<_>>();
    let undone = states.len() - 1 - at;
    Rebuilt { steps, undone }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ImageId;
    use helios_core::ContentHash;

    fn image() -> ImageId {
        ImageId(ContentHash([7; 32]))
    }

    /// A line of versions, each `(exposure, kind)` continuing the last.
    fn line(steps: &[(f64, Option<StepKind>)]) -> Vec<Version> {
        let mut out: Vec<Version> = Vec::new();
        for (i, (ev, kind)) in steps.iter().enumerate() {
            let parents = out.last().map(|v| vec![v.id]).unwrap_or_default();
            let v = Version::new(image(), parents, json!({ "ev": ev }), "user", i as u64)
                .labelled(&format!("step {i}"))
                .with_meta(kind.map(|k| marked(None, k)));
            out.push(v);
        }
        out
    }

    fn evs(steps: &[RebuiltStep]) -> Vec<f64> {
        steps
            .iter()
            .map(|s| s.version.edit["ev"].as_f64().unwrap())
            .collect()
    }

    use StepKind::*;

    #[test]
    fn unmarked_versions_read_as_steps() {
        let l = line(&[(1.0, None), (2.0, None), (3.0, None)]);
        let r = rebuild(&l);
        assert_eq!(evs(r.undo()), [1.0, 2.0, 3.0]);
        assert!(r.redo().is_empty());
        assert_eq!(r.steps[0].before, None, "the first starts unedited");
        assert_eq!(r.steps[1].before, Some(&json!({ "ev": 1.0 })));
    }

    #[test]
    fn an_undo_saved_is_not_undone_again() {
        let l = line(&[(1.0, Some(Do)), (2.0, Some(Do)), (1.0, Some(Undo))]);
        let r = rebuild(&l);
        assert_eq!(evs(r.undo()), [1.0]);
        assert_eq!(evs(r.redo()), [2.0], "the undone step is a redo");
    }

    #[test]
    fn a_redo_and_a_new_step_after_an_undo() {
        let l = line(&[
            (1.0, Some(Do)),
            (2.0, Some(Do)),
            (1.0, Some(Undo)),
            (2.0, Some(Redo)),
        ]);
        let r = rebuild(&l);
        assert_eq!(evs(r.undo()), [1.0, 2.0]);
        assert!(r.redo().is_empty());
        let l = line(&[
            (1.0, Some(Do)),
            (2.0, Some(Do)),
            (1.0, Some(Undo)),
            (5.0, Some(Do)),
        ]);
        let r = rebuild(&l);
        assert_eq!(
            evs(r.undo()),
            [1.0, 5.0],
            "the new step replaced the undone"
        );
        assert!(r.redo().is_empty());
    }

    #[test]
    fn several_undos_saved_as_one_version_move_back_as_far() {
        let l = line(&[
            (1.0, Some(Do)),
            (2.0, Some(Do)),
            (3.0, Some(Do)),
            (1.0, Some(Undo)),
        ]);
        let r = rebuild(&l);
        assert_eq!(evs(r.undo()), [1.0]);
        assert_eq!(evs(r.redo()), [2.0, 3.0], "the next redo first");
    }

    #[test]
    fn undo_back_to_the_unedited_image() {
        // The unedited image has no version: an undo that reaches no
        // step below reached it.
        let l = line(&[
            (1.0, Some(Do)),
            (2.0, Some(Do)),
            (1.0, Some(Undo)),
            (0.0, Some(Undo)),
        ]);
        let r = rebuild(&l);
        assert!(r.undo().is_empty());
        assert_eq!(evs(r.redo()), [1.0, 2.0]);
        assert_eq!(r.redo()[0].before, None);
    }

    #[test]
    fn progress_and_cancel_are_no_step() {
        let l = line(&[
            (1.0, Some(Do)),
            (1.5, Some(Progress)),
            (2.0, Some(Do)),
            (2.5, Some(Progress)),
            (2.0, Some(Cancel)),
        ]);
        let r = rebuild(&l);
        assert_eq!(evs(r.undo()), [1.0, 2.0]);
        assert_eq!(
            r.steps[1].before,
            Some(&json!({ "ev": 1.0 })),
            "a gesture starts from before its saves in progress"
        );
        // An older line, no markers: the label says in progress.
        let mut old = line(&[(1.0, None), (1.5, None), (2.0, None)]);
        old[1].label = Some("Exposure (in progress)".into());
        assert_eq!(evs(rebuild(&old).undo()), [1.0, 2.0]);
    }

    #[test]
    fn a_replay_to_no_known_state_reads_as_a_step() {
        let l = line(&[(1.0, Some(Do)), (2.0, Some(Do)), (9.0, Some(Redo))]);
        let r = rebuild(&l);
        assert_eq!(evs(r.undo()), [1.0, 2.0, 9.0]);
        let l = line(&[(1.0, Some(Do)), (2.0, Some(Do)), (9.0, Some(Undo))]);
        assert_eq!(evs(rebuild(&l).undo()), [1.0, 2.0, 9.0]);
    }

    #[test]
    fn the_marker_keeps_the_rest_of_the_meta() {
        let m = marked(Some(json!({ "workshop_session": "s1" })), Undo);
        assert_eq!(m["workshop_session"], "s1");
        assert_eq!(m[STEP]["kind"], "undo");
        let v = Version::new(image(), vec![], json!({}), "user", 0).with_meta(Some(m));
        assert_eq!(StepKind::of(&v), Undo);
    }
}
