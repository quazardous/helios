//! `egui_hud`: controls as data, between an application's parameters
//! and whatever the person touches. It knows nothing of the application:
//! values come through [`value::ValueStore`], parameters are described by
//! [`value::ParamDescriptor`], and the `controls.*` commands are plain
//! functions a host declares in its own command registry.
//!
//! - [`model`]: what a control is (purpose, value, units, ranges, feel,
//!   detents, read-out), the rack standard (connectors, cases and their
//!   chrome, variants, indicators), modes and representations. No egui.
//! - [`slot`]: what a control asks of a boxed surface: slots, size
//!   policies per representation, orientations, integration levels.
//! - [`card`]: a card or a case laid out on the slot engine
//!   (`egui_gridflow`), item by item.
//! - [`context`]: the context keys (task, space, detail, image, tool,
//!   device), the box and the density, each key's class, a part's tier,
//!   and the rules a card or a case declares over the keys.
//! - [`resolve`]: a card resolved in its context: the precedence (the
//!   declaration, the rules, the profile, the person's layout, the
//!   moment), the slot engine, a placement per item with its reasons, the
//!   "More (n)" entry, anti-jump, and the reachability invariant.
//! - [`registry`]: every control registered and described as data, the
//!   patches onto parameters, implicit controls built from descriptors
//!   for the parameters nothing is patched to.
//! - [`value`]: the value type, the parameter descriptor, the store
//!   traits and a store in memory.
//! - [`flow`]: begin, preview, commit (one undo step, through the
//!   vetoable `controls.change` step) or cancel (no trace).
//! - [`commands`]: `controls.catalogue`, `controls.describe`,
//!   `controls.set`, `controls.nudge`, `controls.reset`,
//!   `controls.context`, `controls.find`, `controls.reveal`,
//!   `controls.reveal_all`, and what a `controls` collector reads.
//! - `render` (feature `render`): the renderers on egui-pop.
//! - `bench` (feature `bench`): a shelf of a host's controls composed
//!   onto a card and tried live, and its page.

#[cfg(feature = "bench")]
pub mod bench;
pub mod card;
pub mod commands;
pub mod context;
pub mod flow;
pub mod model;
pub mod registry;
#[cfg(feature = "render")]
pub mod render;
pub mod resolve;
pub mod slot;
pub mod value;

pub use context::{ContextKeys, Rule, Tier};
pub use model::{Control, Kind, Mode, Representation, Surface, Variant};
pub use registry::{Patch, Registry, implicit};
pub use resolve::{CardPlacement, Layout, Placement, Visibility, resolve};
pub use slot::{Integration, Orientation, SizePolicy, Slot};
pub use value::{
    MemoryStore, Param, ParamDescriptor, ParamType, Value, ValueSource, ValueStore, Widget,
};
