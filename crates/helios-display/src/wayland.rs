//! The Wayland side of the probe: the registry read, the colour manager
//! bound if the compositor announces it, and what it says it supports.

use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::color_management::v1::client::wp_color_management_output_v1::{
    self, WpColorManagementOutputV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_color_manager_v1::{
    self, WpColorManagerV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_image_description_info_v1::{
    self, WpImageDescriptionInfoV1,
};
use wayland_protocols::wp::color_management::v1::client::wp_image_description_v1::{
    self, WpImageDescriptionV1,
};

use crate::{
    Chromaticities, ColorManagement, ScreenDescription, feature_name, intent_name, primaries_name,
    transfer_name,
};

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

// --- Each output's image description ---

/// The outputs' read: the globals, then each output's name and image
/// description, by the output's index.
#[derive(Default)]
struct Outputs {
    manager: Option<(u32, u32)>,
    /// Each `wl_output` global: its name and version.
    globals: Vec<(u32, u32)>,
    found: Vec<ScreenDescription>,
    /// Each output's image description, once ready.
    ready: Vec<bool>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Outputs {
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
        {
            if interface == MANAGER {
                state.manager = Some((name, version));
            } else if interface == "wl_output" {
                state.globals.push((name, version));
            }
        }
    }
}

impl Dispatch<WpColorManagerV1, ()> for Outputs {
    fn event(
        _: &mut Self,
        _: &WpColorManagerV1,
        _: wp_color_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, usize> for Outputs {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.found[*i].name = Some(name);
        }
    }
}

impl Dispatch<WpColorManagementOutputV1, usize> for Outputs {
    fn event(
        _: &mut Self,
        _: &WpColorManagementOutputV1,
        _: wp_color_management_output_v1::Event,
        _: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpImageDescriptionV1, usize> for Outputs {
    fn event(
        state: &mut Self,
        _: &WpImageDescriptionV1,
        event: wp_image_description_v1::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_image_description_v1::Event::Failed { cause, msg } => {
                state.found[*i].error = Some(format!("cause {}: {msg}", u32::from(cause)));
            }
            wp_image_description_v1::Event::Ready { .. }
            | wp_image_description_v1::Event::Ready2 { .. } => state.ready[*i] = true,
            _ => {}
        }
    }
}

impl Dispatch<WpImageDescriptionInfoV1, usize> for Outputs {
    fn event(
        state: &mut Self,
        _: &WpImageDescriptionInfoV1,
        event: wp_image_description_info_v1::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wp_image_description_info_v1::Event as E;
        let d = &mut state.found[*i];
        match event {
            E::Primaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => {
                d.primaries = Some(Chromaticities {
                    red: (r_x, r_y),
                    green: (g_x, g_y),
                    blue: (b_x, b_y),
                    white: (w_x, w_y),
                });
            }
            E::TargetPrimaries {
                r_x,
                r_y,
                g_x,
                g_y,
                b_x,
                b_y,
                w_x,
                w_y,
            } => {
                d.target_primaries = Some(Chromaticities {
                    red: (r_x, r_y),
                    green: (g_x, g_y),
                    blue: (b_x, b_y),
                    white: (w_x, w_y),
                });
            }
            E::PrimariesNamed { primaries } => {
                d.primaries_named = Some(primaries_name(u32::from(primaries)));
            }
            E::TfNamed { tf } => d.transfer = Some(transfer_name(u32::from(tf))),
            E::TfPower { eexp } if d.transfer.is_none() => {
                d.transfer = Some(format!("power {}", f64::from(eexp) / 10_000.0));
            }
            E::Luminances {
                max_lum,
                reference_lum,
                ..
            } => {
                d.max_luminance = Some(max_lum);
                d.reference_luminance = Some(reference_lum);
            }
            E::TargetLuminance { max_lum, .. } => d.target_max_luminance = Some(max_lum),
            E::IccFile { .. } => d.icc = true,
            _ => {}
        }
    }
}

/// Each output's image description, read on a connection of its own:
/// the outputs bound, each one's colour-management object asked for its
/// description, the information of those ready read, everything
/// destroyed. Empty when the compositor offers no colour management.
pub fn screens() -> Result<Vec<ScreenDescription>, String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("cannot connect to the compositor: {e}"))?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let registry = conn.display().get_registry(&qh, ());
    let mut state = Outputs::default();
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("the compositor's globals: {e}"))?;
    let Some((manager_name, manager_version)) = state.manager else {
        return Ok(Vec::new());
    };
    // Version 2 sends `ready2`, version 3 adds nothing read here.
    let manager: WpColorManagerV1 = registry.bind(manager_name, manager_version.min(3), &qh, ());
    let globals = std::mem::take(&mut state.globals);
    state.found = vec![ScreenDescription::default(); globals.len()];
    state.ready = vec![false; globals.len()];
    // `wl_output.name` comes with version 4.
    let outputs: Vec<wl_output::WlOutput> = globals
        .iter()
        .enumerate()
        .map(|(i, (name, version))| registry.bind(*name, (*version).min(4), &qh, i))
        .collect();
    let colour: Vec<WpColorManagementOutputV1> = outputs
        .iter()
        .enumerate()
        .map(|(i, o)| manager.get_output(o, &qh, i))
        .collect();
    let descriptions: Vec<WpImageDescriptionV1> = colour
        .iter()
        .enumerate()
        .map(|(i, c)| c.get_image_description(&qh, i))
        .collect();
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("the outputs' descriptions: {e}"))?;
    for (i, d) in descriptions.iter().enumerate() {
        if state.ready[i] {
            d.get_information(&qh, i);
        }
    }
    queue
        .roundtrip(&mut state)
        .map_err(|e| format!("the descriptions' information: {e}"))?;
    for d in descriptions {
        d.destroy();
    }
    for c in colour {
        c.destroy();
    }
    for o in outputs {
        if o.version() >= 3 {
            o.release();
        }
    }
    manager.destroy();
    let _ = conn.flush();
    Ok(state.found)
}
