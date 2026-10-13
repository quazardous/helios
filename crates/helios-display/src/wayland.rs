//! The Wayland side of the probe: the registry read, the colour manager
//! bound if the compositor announces it, and what it says it supports.

use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::color_management::v1::client::wp_color_manager_v1::{
    self, WpColorManagerV1,
};

use crate::{ColorManagement, feature_name, intent_name, primaries_name, transfer_name};

/// The protocol's global.
const MANAGER: &str = "wp_color_manager_v1";

#[derive(Default)]
struct State {
    /// The manager's global name and version, once announced.
    manager: Option<(u32, u32)>,
    found: ColorManagement,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == MANAGER
        {
            state.manager = Some((name, version));
        }
    }
}

impl Dispatch<WpColorManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &WpColorManagerV1,
        event: wp_color_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let found = &mut state.found;
        match event {
            wp_color_manager_v1::Event::SupportedIntent { render_intent } => {
                found.intents.push(intent_name(u32::from(render_intent)));
            }
            wp_color_manager_v1::Event::SupportedFeature { feature } => {
                found.features.push(feature_name(u32::from(feature)));
            }
            wp_color_manager_v1::Event::SupportedTfNamed { tf } => {
                found.transfer_functions.push(transfer_name(u32::from(tf)));
            }
            wp_color_manager_v1::Event::SupportedPrimariesNamed { primaries } => {
                found.primaries.push(primaries_name(u32::from(primaries)));
            }
            _ => {}
        }
    }
}

/// What the compositor's colour manager supports, `None` when it does
/// not announce one.
pub fn color_management() -> Result<Option<ColorManagement>, String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("cannot connect to the compositor: {e}"))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let registry = conn.display().get_registry(&qh, ());
    let mut state = State::default();
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("the compositor's globals: {e}"))?;
    let Some((name, version)) = state.manager else {
        return Ok(None);
    };
    // Version 1 announces everything read here.
    let manager: WpColorManagerV1 = registry.bind(name, 1, &qh, ());
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("the colour manager's announcements: {e}"))?;
    manager.destroy();
    let _ = conn.flush();
    state.found.version = version;
    Ok(Some(state.found))
}
