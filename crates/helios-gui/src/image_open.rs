//! An image opened or closed in the darkroom: requests on the bus.
//!
//! Opening another image, or closing the one open, passes once through a
//! request point ([`ImageOpen`], `image.open`; [`ImageClose`],
//! `image.close`), its listeners in the order [`ORDER`] declares: the edit
//! of the image open saved first (`edit.save`: the snapshots pending at
//! the step it saves pinned and kept with it; a save that fails refuses
//! the request with its reason, and the image stays open), then the edit's
//! history leaves the image (`history.scope`: the image's steps of the
//! value store and the snapshot book cleared, so the next image's undo
//! never walks into this one's steps; the workspace's steps, graph edits,
//! stay). Only a request that went through is acted on: the
//! next image loads (its saved edit, the viewport's network: pixels, the
//! listed exception), and `image.closed` / `image.opened` are said on the
//! journal with the listeners that ran (`ran`). A listener refuses with
//! `HookError::refuse(reason)` and the request stops there. Both points
//! are declared fail-closed (bus-contract's veto): a listener that fails
//! (an error, a panic, past its deadline: [`SAVE_DEADLINE`] for the save)
//! refuses the request too, and the listeners after it do not run.
//!
//! Before, the order lived in a comment at the one place that switched
//! images (`persist.close_image` first, "what is drawn now is not saved
//! into its edit"); any other path that opened an image skipped the save.
//! The listeners hold the edit's state and the interface's (`Rc`): they
//! are local listeners ([`listen_local`]), bound to the interface's
//! thread, where the request is passed.
//!
//! Once the next image is loaded, what follows the image open passes once
//! through `image.opened` ([`ImageOpened`]; `image.closed`,
//! [`ImageClosed`], when no image follows), its followers in the order
//! [`FOLLOWERS`] declares: the image's pinned snapshots loaded
//! (`snapshots.book`), the labels and locks its edit was saved with
//! (`mask.marks`), the workshop session's photo (`workshop.photo`). Each
//! follower holds its own state behind a handle (`Rc` or `Arc`) and does
//! its part inside the pass ([`listen_follower_local`]); none compares
//! the image open at each pass.

use std::rc::Rc;

use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, PointKind, Refused,
};
use serde_json::{Value, json};

/// The request to open an image in the darkroom.
pub const OPEN: &str = "helios.image.open";
/// The request to close the image open in the darkroom.
pub const CLOSE: &str = "helios.image.close";
/// The journal's event once an image is open.
pub const OPENED: &str = "helios.image.opened";
/// The journal's event once the image open before is closed.
pub const CLOSED: &str = "helios.image.closed";
/// The journal's event when a listener refused the request.
pub const REFUSED: &str = "image.refused";
/// The owner of the desktop's listeners of both points.
pub const BUS_OWNER: &str = "helios.darkroom";

/// The edit of the image open saved, its write answered, and the
/// snapshots pending at its step kept: a refused write refuses the
/// request.
pub const SAVE: &str = "helios.edit.save";
/// The edit's history (the image's steps of the value store, the snapshot
/// book) leaves the image; the workspace's steps stay.
pub const SCOPE: &str = "helios.history.scope";

/// How long `edit.save` may run before the bus counts it late (a late
/// listener refuses the switch). The save waits up to 30 s for the
/// library to answer its step (`persist`'s `SAVE_WAIT`, itself above the
/// vault's 5 s wait for a busy database and a slow disk's sync), then
/// fails with its own reason: the deadline is above that wait, so a save
/// that ends in time on a slow disk is never refused as late, and only a
/// save stuck past its own wait is.
pub const SAVE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(40);

/// The listeners of both points, each with the one it runs after: the
/// order they run in.
pub const ORDER: &[(&str, Option<&str>)] = &[(SAVE, None), (SCOPE, Some(SAVE))];

/// The image's pinned snapshots loaded into the snapshot book, with the
/// parameters its edit saves.
pub const SNAPSHOTS: &str = "helios.snapshots.book";
/// The labels and locks the image's edit was saved with, put back in the
/// value store (another image's gone).
pub const MARKS: &str = "helios.mask.marks";
/// The workshop session's photo: the vault image its steps are recorded
/// on.
pub const WORKSHOP: &str = "helios.workshop.photo";

/// The followers of [`ImageOpened`] and [`ImageClosed`], each with the
/// one it runs after: the order they run in.
pub const FOLLOWERS: &[(&str, Option<&str>)] = &[
    (SNAPSHOTS, None),
    (MARKS, Some(SNAPSHOTS)),
    (WORKSHOP, Some(MARKS)),
];

/// What a switch is: the image open (none when nothing is edited) and the
/// one asked (none for a close), as the listeners read it.
#[derive(Clone, Debug, Default)]
pub struct Switch {
    /// The image open, by its id's hex.
    pub from: Option<String>,
    /// The image asked, by its id's hex; none for a close.
    pub to: Option<String>,
    /// When it is asked, in seconds: the time a step saved is dated.
    pub at: f64,
}

pub use bus_contract::Refusal;

/// What the listeners did, in order; once one refused, the others are
/// not run.
#[derive(Clone, Debug, Default)]
pub struct Switched {
    /// The listeners that went through, in their order (one that
    /// refused is named by [`Switched::refused`], not here).
    pub ran: Vec<String>,
    /// The request was refused: the listener, why, and whether it
    /// failed rather than refused (the bus's [`Refusal`]).
    pub refused: Option<Refusal>,
    /// The step `edit.save` wrote: the image, and whether its edit
    /// changes it from its original.
    pub saved: Option<(String, bool)>,
}

/// The request to open an image in the darkroom.
pub struct ImageOpen;

impl Point for ImageOpen {
    const ID: &'static str = OPEN;
    type Input = Switch;
    type Value = Switched;
    const KIND: PointKind = PointKind::Request;
}

/// The request to close the image open in the darkroom.
pub struct ImageClose;

impl Point for ImageClose {
    const ID: &'static str = CLOSE;
    type Input = Switch;
    type Value = Switched;
    const KIND: PointKind = PointKind::Request;
}

/// What the followers read once an image is open, or closed with none
/// after it.
#[derive(Clone, Debug, Default)]
pub struct Shown {
    /// The image open now, by its id's hex; none once closed.
    pub image: Option<String>,
    /// Its pinned snapshots, each with the values of its version.
    pub snapshots: Vec<crate::edit_snapshots::Taken>,
    /// The parameters its edit saves, by node.
    pub saved: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    /// The labels and locks its edit was saved with (null: none).
    pub marks: Value,
}

/// The followers that ran, in order.
#[derive(Clone, Debug, Default)]
pub struct Followed {
    pub ran: Vec<String>,
}

/// An image is open in the darkroom: what follows it, in order.
pub struct ImageOpened;

impl Point for ImageOpened {
    const ID: &'static str = OPENED;
    type Input = Shown;
    type Value = Followed;
}

/// The image open is closed, none after it: what follows it, in order.
pub struct ImageClosed;

impl Point for ImageClosed {
    const ID: &'static str = CLOSED;
    type Input = Shown;
    type Value = Followed;
}

/// The darkroom's points, this crate's contract with the application's
/// bus: declared by [`declare`].
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<ImageOpen>(
        "Open an image in the darkroom: the edit open saved first; a failed save refuses",
    )
    .fail_closed(),
    PointDecl::of::<ImageClose>(
        "Close the image open in the darkroom: its edit saved first; a failed save refuses",
    )
    .fail_closed(),
    PointDecl::of::<ImageOpened>(
        "An image is open in the darkroom: what follows the image open, once, in order",
    )
    .budget(crate::INTERFACE_PASS_BUDGET),
    PointDecl::of::<ImageClosed>(
        "The image open is closed, none after it: what follows the image open, once, in order",
    ),
];

/// Declares [`POINTS`] on `bus` (once; a second desktop on the same bus
/// finds them there).
pub fn declare(bus: &dyn Bus) {
    for refused in bus_contract::declare_all(bus, POINTS) {
        if !matches!(refused, Refused::AlreadyDeclared { .. }) {
            log::error!(target: "helios", "darkroom: {refused}");
        }
    }
}

/// Registers `run` as the listener `id` (one of [`ORDER`]'s) of both
/// [`ImageOpen`] and [`ImageClose`] on `bus`, in its declared place: a
/// local listener bound to the calling thread (the interface's). `run`
/// refuses the request with `Err(reason)` (`HookError::refuse`); once one
/// refused or failed, the listeners after it are not run. One there
/// already under `id` is replaced.
///
/// # Errors
/// `id` is not in [`ORDER`], or the bus refused it.
pub fn listen_local(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&Switch, &mut Switched) -> Result<(), String> + 'static,
) -> Result<(), String> {
    let Some(&(_, after)) = ORDER.iter().find(|(listener, _)| *listener == id) else {
        return Err(format!("`{id}` is not a listener of {OPEN}"));
    };
    declare(bus);
    let opts = || {
        let mut opts = ListenOpts::new().owner(BUS_OWNER);
        if id == SAVE {
            opts = opts.deadline(SAVE_DEADLINE);
        }
        match after {
            Some(after) => opts.after(after),
            None => opts,
        }
    };
    let run = Rc::new(move |switch: &Switch, switched: &mut Switched| {
        run(switch, switched).map_err(HookError::refuse)?;
        switched.ran.push(id.to_owned());
        Ok::<(), HookError>(())
    });
    bus.unlisten(OPEN, id);
    bus.unlisten(CLOSE, id);
    let open = run.clone();
    bus.listen_local::<ImageOpen>(id, opts(), move |s, v| open(s, v))
        .map_err(|e| e.to_string())?;
    bus.listen_local::<ImageClose>(id, opts(), move |s, v| run(s, v))
        .map_err(|e| e.to_string())
}

/// Passes `switch` through `image.open` (an image asked) or `image.close`
/// (none) on `bus`: what the listeners did, in order.
///
/// Fail-closed (both points are declared so): a listener that refused
/// or failed (an error, a panic, past its deadline) refuses the switch,
/// named in [`Switched::refused`], and the listeners after it do not run.
pub fn pass(bus: &dyn Bus, switch: Switch) -> Switched {
    declare(bus);
    let altered = if switch.to.is_some() {
        bus.alter::<ImageOpen>(switch, Switched::default())
    } else {
        bus.alter::<ImageClose>(switch, Switched::default())
    };
    for failure in &altered.failures {
        log::warn!(target: "helios", "{failure}");
    }
    let mut switched = altered.value;
    switched.refused = altered.refused;
    switched
}

/// Registers `run` as the follower `id` (one of [`FOLLOWERS`]) of both
/// [`ImageOpened`] and [`ImageClosed`] on `bus`, in its declared place: a
/// local listener bound to the calling thread (the interface's). One
/// there already under `id` is replaced.
///
/// # Errors
/// `id` is not in [`FOLLOWERS`], or the bus refused it.
pub fn listen_follower_local(
    bus: &dyn Bus,
    id: &'static str,
    run: impl Fn(&Shown) + 'static,
) -> Result<(), String> {
    let Some(&(_, after)) = FOLLOWERS.iter().find(|(listener, _)| *listener == id) else {
        return Err(format!("`{id}` is not a follower of {OPENED}"));
    };
    declare(bus);
    let opts = || {
        let opts = ListenOpts::new().owner(BUS_OWNER);
        match after {
            Some(after) => opts.after(after),
            None => opts,
        }
    };
    let run = Rc::new(move |shown: &Shown, followed: &mut Followed| {
        followed.ran.push(id.to_owned());
        run(shown);
        Ok::<(), HookError>(())
    });
    bus.unlisten(OPENED, id);
    bus.unlisten(CLOSED, id);
    let opened = run.clone();
    bus.listen_local::<ImageOpened>(id, opts(), move |s, v| opened(s, v))
        .map_err(|e| e.to_string())?;
    bus.listen_local::<ImageClosed>(id, opts(), move |s, v| run(s, v))
        .map_err(|e| e.to_string())
}

/// Passes `shown` through `image.opened` (an image open) or
/// `image.closed` (none) on `bus`: the followers that ran, in order.
pub fn follow(bus: &dyn Bus, shown: Shown) -> Followed {
    let (point, altered) = if shown.image.is_some() {
        (OPENED, bus.alter::<ImageOpened>(shown, Followed::default()))
    } else {
        (CLOSED, bus.alter::<ImageClosed>(shown, Followed::default()))
    };
    for failure in &altered.failures {
        log::warn!(target: "helios", "{point}: {failure}");
    }
    altered.value
}

/// The order the darkroom's points' listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[OPEN, CLOSE, OPENED, CLOSED])
}

/// The journal's words for a pass: the images, the listeners that ran,
/// and the refusal if any.
pub fn said(switch: &Switch, switched: &Switched) -> Value {
    let mut data = json!({
        "from": switch.from,
        "to": switch.to,
        "ran": switched.ran,
    });
    if let Some(r) = &switched.refused {
        data["refused"] = json!({ "by": r.listener, "reason": r.reason, "failed": r.failed });
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn a_refusal_stops_the_listeners_after_it() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        let scoped = Rc::new(RefCell::new(0));
        let s = scoped.clone();
        listen_local(&bus, SCOPE, move |_, _| {
            *s.borrow_mut() += 1;
            Ok(())
        })
        .unwrap();
        listen_local(&bus, SAVE, |switch, _| match switch.to.as_deref() {
            Some("refuse") => Err("not saved".into()),
            _ => Ok(()),
        })
        .unwrap();
        let ok = pass(
            &bus,
            Switch {
                from: Some("a".into()),
                to: Some("b".into()),
                at: 0.0,
            },
        );
        assert_eq!(ok.ran, [SAVE, SCOPE]);
        assert_eq!(ok.refused, None);
        let closed = pass(&bus, Switch::default());
        assert_eq!(closed.ran, [SAVE, SCOPE]);
        let refused = pass(
            &bus,
            Switch {
                from: Some("a".into()),
                to: Some("refuse".into()),
                at: 0.0,
            },
        );
        assert!(refused.ran.is_empty(), "{:?}", refused.ran);
        assert_eq!(
            refused.refused,
            Some(Refusal::new(
                OPEN,
                SAVE,
                Some(BUS_OWNER.into()),
                "not saved"
            ))
        );
        assert_eq!(*scoped.borrow(), 2);
        assert!(listen_local(&bus, "nobody", |_, _| Ok(())).is_err());
    }

    /// The points are fail-closed (bus-contract's veto): a save that
    /// panics refuses the switch as failed, and `history.scope` after it
    /// does not run, so the image's undo history stays.
    #[test]
    fn a_failing_listener_refuses_and_stops_the_listeners_after_it() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        let scoped = Rc::new(RefCell::new(0));
        let s = scoped.clone();
        listen_local(&bus, SCOPE, move |_, _| {
            *s.borrow_mut() += 1;
            Ok(())
        })
        .unwrap();
        listen_local(&bus, SAVE, |_, _| panic!("the disk went away")).unwrap();
        let switched = pass(
            &bus,
            Switch {
                from: Some("a".into()),
                to: Some("b".into()),
                at: 0.0,
            },
        );
        let refused = switched.refused.expect("a failed save refuses");
        assert_eq!(refused.listener, SAVE);
        assert!(refused.failed);
        assert!(refused.reason.contains("the disk went away"), "{refused}");
        assert_eq!(*scoped.borrow(), 0, "nothing after a failed listener runs");
        let said = said(
            &Switch::default(),
            &Switched {
                refused: Some(refused),
                ..Switched::default()
            },
        );
        assert_eq!(said["refused"]["by"], SAVE);
        assert_eq!(said["refused"]["failed"], true);
    }

    /// A listener past its deadline refuses the switch, even when it
    /// answered yes: its answer came too late to be read.
    #[test]
    fn a_late_listener_refuses() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::with_default_deadline(
            std::time::Duration::from_millis(20),
        );
        listen_local(&bus, SCOPE, |_, _| {
            std::thread::sleep(std::time::Duration::from_millis(60));
            Ok(())
        })
        .unwrap();
        let switched = pass(&bus, Switch::default());
        let refused = switched.refused.expect("a late listener refuses");
        assert_eq!(refused.listener, SCOPE);
        assert!(refused.failed);
        assert!(refused.reason.contains("deadline"), "{refused}");
    }

    #[test]
    fn each_follower_runs_once_per_pass_in_its_declared_order() {
        let bus = egui_pop::api::bus::egui_pop_hooks::Kernel::new();
        let runs = Rc::new(RefCell::new(Vec::new()));
        // Registered against their order: the bus places them.
        for id in [WORKSHOP, MARKS, SNAPSHOTS] {
            let r = runs.clone();
            listen_follower_local(&bus, id, move |shown| {
                r.borrow_mut().push((id, shown.image.clone()));
            })
            .unwrap();
        }
        let opened = follow(
            &bus,
            Shown {
                image: Some("b".into()),
                ..Shown::default()
            },
        );
        assert_eq!(opened.ran, [SNAPSHOTS, MARKS, WORKSHOP]);
        let closed = follow(&bus, Shown::default());
        assert_eq!(closed.ran, [SNAPSHOTS, MARKS, WORKSHOP]);
        let b = Some("b".to_string());
        assert_eq!(
            *runs.borrow(),
            [
                (SNAPSHOTS, b.clone()),
                (MARKS, b.clone()),
                (WORKSHOP, b),
                (SNAPSHOTS, None),
                (MARKS, None),
                (WORKSHOP, None),
            ]
        );
        assert!(listen_follower_local(&bus, SAVE, |_| ()).is_err());
    }
}
