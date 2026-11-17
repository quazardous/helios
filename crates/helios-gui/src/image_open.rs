//! An image opened or closed in the darkroom: requests on the bus.
//!
//! Opening another image, or closing the one open, passes once through a
//! request point ([`ImageOpen`], `image.open`; [`ImageClose`],
//! `image.close`), its listeners in the order [`ORDER`] declares: the edit
//! of the image open saved first (`edit.save`: a save that fails refuses
//! the request with its reason, and the image stays open), then the edit's
//! history leaves the image (`history.scope`: the value store's steps and
//! the snapshot book cleared, so the next image's undo never walks into
//! this one's steps). Only a request that went through is acted on: the
//! next image loads (its saved edit, the viewport's network: pixels, the
//! listed exception), and `image.closed` / `image.opened` are said on the
//! journal with the listeners that ran (`ran`).
//!
//! Before, the order lived in a comment at the one place that switched
//! images (`persist.close_image` first, "what is drawn now is not saved
//! into its edit"); any other path that opened an image skipped the save.
//! The listeners hold the edit's state and the interface's (`Rc`): they
//! are local listeners ([`listen_local`]), bound to the interface's
//! thread, where the request is passed.

use std::rc::Rc;

use egui_pop::api::bus::bus_contract::{
    self, Bus, HookError, ListenOpts, Point, PointDecl, PointKind, Refused,
};
use serde_json::{Value, json};

/// The request to open an image in the darkroom.
pub const OPEN: &str = "image.open";
/// The request to close the image open in the darkroom.
pub const CLOSE: &str = "image.close";
/// The journal's event once an image is open.
pub const OPENED: &str = "image.opened";
/// The journal's event once the image open before is closed.
pub const CLOSED: &str = "image.closed";
/// The journal's event when a listener refused the request.
pub const REFUSED: &str = "image.refused";
/// The owner of the desktop's listeners of both points.
pub const BUS_OWNER: &str = "helios.darkroom";

/// The edit of the image open saved, its write answered: a refused write
/// refuses the request.
pub const SAVE: &str = "edit.save";
/// The edit's history (the value store's steps, the snapshot book) leaves
/// the image.
pub const SCOPE: &str = "history.scope";

/// The listeners of both points, each with the one it runs after: the
/// order they run in.
pub const ORDER: &[(&str, Option<&str>)] = &[(SAVE, None), (SCOPE, Some(SAVE))];

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

/// A refusal: the listener that refused, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub by: String,
    pub reason: String,
}

/// What the listeners did, in order; once one refused, the others are
/// not run.
#[derive(Clone, Debug, Default)]
pub struct Switched {
    /// The listeners that ran, in their order.
    pub ran: Vec<String>,
    /// The request was refused, by whom and why.
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

/// The darkroom's points, this crate's contract with the application's
/// bus: declared by [`declare`].
pub const POINTS: &[PointDecl] = &[
    PointDecl::of::<ImageOpen>(
        "Open an image in the darkroom: the edit open saved first; a failed save refuses",
    ),
    PointDecl::of::<ImageClose>(
        "Close the image open in the darkroom: its edit saved first; a failed save refuses",
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
/// refuses the request with `Err(reason)`; once one refused, the
/// listeners after it are not run. One there already under `id` is
/// replaced.
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
        let opts = ListenOpts::new().owner(BUS_OWNER);
        match after {
            Some(after) => opts.after(after),
            None => opts,
        }
    };
    let run = Rc::new(move |switch: &Switch, switched: &mut Switched| {
        if switched.refused.is_some() {
            return Ok::<(), HookError>(());
        }
        switched.ran.push(id.to_owned());
        if let Err(reason) = run(switch, switched) {
            switched.refused = Some(Refusal {
                by: id.to_owned(),
                reason,
            });
        }
        Ok(())
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
pub fn pass(bus: &dyn Bus, switch: Switch) -> Switched {
    let (point, altered) = if switch.to.is_some() {
        (OPEN, bus.alter::<ImageOpen>(switch, Switched::default()))
    } else {
        (CLOSE, bus.alter::<ImageClose>(switch, Switched::default()))
    };
    for failure in &altered.failures {
        log::warn!(target: "helios", "{point}: {failure}");
    }
    altered.value
}

/// The order both points' listeners run in on `bus`.
pub fn order(bus: &dyn Bus) -> String {
    bus_contract::order_dump(bus, &[OPEN, CLOSE])
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
        data["refused"] = json!({ "by": r.by, "reason": r.reason });
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
        assert_eq!(refused.ran, [SAVE]);
        assert_eq!(
            refused.refused,
            Some(Refusal {
                by: SAVE.into(),
                reason: "not saved".into()
            })
        );
        assert_eq!(*scoped.borrow(), 2);
        assert!(listen_local(&bus, "nobody", |_, _| Ok(())).is_err());
    }
}
