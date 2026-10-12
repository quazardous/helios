//! Detached panels' windows in a headless harness, each on a screen of
//! its own scale.
//!
//! kittest draws one window, and egui embeds the others in it, at its
//! scale. Here an immediate-viewport renderer runs each detached panel's
//! window in a pass of its own, as eframe does on the desktop, on a screen
//! of the native scale the test gives it, and keeps what it drew; the
//! window renders on the harness's GPU device, with the harness's
//! renderer (the same textures and viewport GPU objects).
// upstream: kittest draws one viewport; this belongs in egui-pop's test
// kit once it settles.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use eframe::egui_wgpu::{RenderState, ScreenDescriptor, wgpu};
use egui::epaint::ClippedShape;
use egui::{Context, Pos2, Rect, Vec2, ViewportId, ViewportInfo};
use egui_kittest::Harness;
use image::RgbaImage;

use crate::Shell;

/// What a detached panel's window drew at its last pass.
#[derive(Clone)]
pub struct Drawn {
    /// Its physical pixels per point.
    pub pixels_per_point: f32,
    /// Its size, in points.
    pub size: Vec2,
    /// What it drew, in its points.
    pub shapes: Vec<ClippedShape>,
}

#[derive(Default)]
struct State {
    scales: HashMap<ViewportId, f32>,
    drawn: HashMap<ViewportId, Drawn>,
}

/// The detached panels' windows of a harness.
pub struct Windows {
    state: Rc<RefCell<State>>,
    rs: RenderState,
}

/// A shell drawn offscreen at `scale`, its detached panels in windows of
/// their own (each at scale 1 until [`Windows::set_scale`]).
pub fn harness_with_windows<'a>(
    shell: Shell,
    size: Vec2,
    scale: f32,
) -> (Harness<'a, Shell>, Windows) {
    let (h, rs) = crate::headless::harness_parts(shell, size, scale);
    h.ctx.set_embed_viewports(false);
    let state = Rc::new(RefCell::new(State::default()));
    let (st, renderer) = (state.clone(), rs.clone());
    Context::set_immediate_viewport_renderer(move |ctx, viewport| {
        let id = viewport.ids.this;
        let size = viewport
            .builder
            .inner_size
            .unwrap_or(Vec2::new(480.0, 360.0));
        let at = viewport.builder.position.unwrap_or(Pos2::ZERO);
        let scale = st.borrow().scales.get(&id).copied().unwrap_or(1.0);
        let mut input = egui::RawInput {
            viewport_id: id,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(ctx.input(|i| i.time)),
            ..Default::default()
        };
        let place = Rect::from_min_size(at, size);
        input.viewports.insert(
            id,
            ViewportInfo {
                native_pixels_per_point: Some(scale),
                inner_rect: Some(place),
                outer_rect: Some(place),
                focused: Some(false),
                ..Default::default()
            },
        );
        let mut ui_cb = viewport.viewport_ui_cb;
        let mut out = ctx.run_ui(input, |ui| ui_cb(ui));
        // The textures are the harness's: its renderer gets them now.
        apply(&renderer, &mut out.textures_delta);
        st.borrow_mut().drawn.insert(
            id,
            Drawn {
                pixels_per_point: out.pixels_per_point,
                size,
                shapes: out.shapes,
            },
        );
    });
    (h, Windows { state, rs })
}

fn apply(rs: &RenderState, delta: &mut egui::TexturesDelta) {
    let mut renderer = rs.renderer.write();
    for (id, images) in std::mem::take(&mut delta.set) {
        for image in images {
            renderer.update_texture(&rs.device, &rs.queue, id, &image);
        }
    }
    for id in std::mem::take(&mut delta.free) {
        renderer.free_texture(&id);
    }
}

impl Windows {
    /// Moves a detached panel's window to a screen of this native scale,
    /// from its next pass.
    pub fn set_scale(&self, panel: &str, scale: f32) {
        self.state
            .borrow_mut()
            .scales
            .insert(window_id(panel), scale);
    }

    /// What a detached panel's window drew at its last pass.
    pub fn drawn(&self, panel: &str) -> Option<Drawn> {
        self.state.borrow().drawn.get(&window_id(panel)).cloned()
    }

    /// A picture of a detached panel's window, in its physical pixels.
    pub fn render(&self, ctx: &Context, panel: &str) -> Result<RgbaImage, String> {
        let drawn = self
            .drawn(panel)
            .ok_or_else(|| format!("`{panel}` has no window of its own"))?;
        let ppp = drawn.pixels_per_point;
        let px = drawn.size * ppp;
        let (w, h) = (px.x.round() as u32, px.y.round() as u32);
        let screen = ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: ppp,
        };
        let primitives = ctx.tessellate(drawn.shapes, ppp);
        let (device, queue) = (&self.rs.device, &self.rs.queue);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("helios detached window"),
        });
        let mut renderer = self.rs.renderer.write();
        let user = renderer.update_buffers(device, queue, &mut encoder, &primitives, &screen);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("helios detached window"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.rs.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("helios detached window"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            renderer.render(&mut pass, &primitives, &screen);
        }
        drop(renderer);
        // Read back, rows padded to wgpu's alignment.
        let row = 4 * w;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("helios detached window"),
            size: u64::from(padded * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            texture.size(),
        );
        queue.submit(user.into_iter().chain(std::iter::once(encoder.finish())));
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_mins(1)),
            })
            .map_err(|e| format!("the GPU did not answer: {e}"))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| format!("the picture could not be read: {e:?}"))?;
        let bgra = matches!(
            self.rs.target_format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            let start = (y * padded) as usize;
            for x in 0..w {
                let p = start + 4 * x as usize;
                let mut px = [data[p], data[p + 1], data[p + 2], data[p + 3]];
                if bgra {
                    px.swap(0, 2);
                }
                img.put_pixel(x, y, image::Rgba(px));
            }
        }
        Ok(img)
    }
}

fn window_id(panel: &str) -> ViewportId {
    let panel = egui_pop::model::ids::PanelId::new(panel).unwrap_or_else(|e| panic!("{e}"));
    egui_pop::api::workbench::window_id(&panel)
}
