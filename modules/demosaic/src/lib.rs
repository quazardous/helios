//! demosaic: the sensor mosaic (one value per photosite, behind a color
//! filter) to camera RGB, three values per pixel.
//!
//! Methods, chosen by the `method` parameter (`auto` by default):
//!
//! - **RCD** (Ratio Corrected Demosaicing), darktable's default for Bayer
//!   patterns, ported from darktable (see `rcd.rs`);
//! - **Markesteijn** one pass, darktable's default for X-Trans, ported
//!   from darktable (see `xtrans.rs`);
//! - **bilinear**, for any filter pattern.
//!
//! `auto` is RCD on a Bayer pattern and Markesteijn on X-Trans, as in
//! darktable. Near the image's edges darktable fills the pixels these
//! methods do not compute with another method; Helios computes them from
//! replicated edges, so the outer 12 pixels may differ from darktable's.
//!
//! Bilinear: each missing color of
//! a photosite is the mean of the nearest photosites of that color: those of
//! the 3 x 3 neighbourhood, else of the 5 x 5 one (some X-Trans positions
//! lack a red or a blue nearby). On a Bayer pattern this is darktable's
//! bilinear fallback. The pattern comes from the camera's description, a
//! declared context input, anchored at the image origin (the loader shifts
//! it to its crop).
//!
//! Small renders (a quarter of full resolution or less: the viewport's
//! first proxy, the 256 px preview the histogram is computed on) skip the
//! full demosaic: each output pixel is the mean of each colour's
//! photosites under it, read from the full-resolution mosaic, as
//! darktable's thumbnail pipes zoom a mosaic straight to their size
//! (`dt_iop_clip_and_zoom_demosaic_half_size_f` and
//! `dt_iop_clip_and_zoom_demosaic_third_size_xtrans_f` in its
//! `src/common/imagebuf.c`/`demosaic.c`, <https://github.com/darktable-org/darktable>).
//! Larger renders demosaic at full resolution, then reduce.
//!
//! Known limit: within two pixels of the image border the framework
//! replicates edge photosites, whose filter color is not the one the
//! pattern gives there; those pixels are approximate.

mod rcd;
mod xtrans;

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamKind, ParamSpec, Params,
    PortSpec, Region, Role, Value,
};
use helios_photo::Cfa;
use helios_photo::camera::fields;
use helios_photo::cfa::Channel;

pub struct Demosaic;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Rcd,
    Markesteijn,
    Bilinear,
}

#[derive(Debug)]
pub struct Plan {
    cfa: Cfa,
    method: Method,
    /// The largest value to expect: darktable's processed maximum, the
    /// largest white-balance coefficient (at least 1). RCD normalizes by it.
    maximum: f32,
    /// The render's scale: below 1 only when the engine asks for the
    /// output straight at a small scale ([`DIRECT_REDUCTION`]).
    scale: f64,
}

/// At a scale of 1/4 or less the output is computed straight from the
/// mosaic, each output pixel the mean of each colour's photosites under
/// it (at least 4 × 4 of them, every colour of a Bayer or X-Trans pattern
/// among them): what a full demosaic reduced as much would average too,
/// at a fraction of its cost. The viewport's proxy (the power of two at
/// or below the fit) and the preview take it; a fit closer than 1/4 still
/// demosaics fully.
pub const DIRECT_REDUCTION: u32 = 4;

fn index(c: Channel) -> usize {
    match c {
        Channel::Red => 0,
        Channel::Green => 1,
        Channel::Blue => 2,
    }
}

impl Module for Demosaic {
    type Plan = Plan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "demosaic".into(),
            // 2: methods, RCD by default on Bayer patterns.
            version: 2,
            description: "Turn the sensor mosaic into camera RGB".into(),
            params: vec![ParamSpec {
                name: "method".into(),
                description: "auto: RCD on Bayer patterns, Markesteijn on X-Trans".into(),
                kind: ParamKind::Choice {
                    options: vec![
                        "auto".into(),
                        "rcd".into(),
                        "markesteijn".into(),
                        "bilinear".into(),
                    ],
                    default: "auto".into(),
                },
            }],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 4)],
            // The white balance gives RCD the largest value to expect.
            context: vec![fields::CFA.into(), fields::WHITE_BALANCE.into()],
            // A reduced mosaic mixes the colours of its photosites.
            needs: Needs {
                overlap: 2,
                full_resolution: true,
                direct_reduction: Some(DIRECT_REDUCTION),
                ..Needs::default()
            },
            kind: ModuleKind::Process,
            family: Family::Detail,
            role: Role::General,
        }
    }

    fn prepare(&self, params: &Params, context: &Context) -> Result<Plan, String> {
        let name = context
            .get(fields::CFA)
            .and_then(|v| v.as_choice())
            .ok_or_else(|| format!("context field `{}`: a filter pattern needed", fields::CFA))?;
        let cfa = Cfa::parse(name)?;
        let bayer = cfa.period() == 2;
        let method = match params.get("method") {
            Some(Value::Choice(m)) if m == "rcd" => {
                if !bayer {
                    return Err("RCD needs a Bayer pattern".into());
                }
                Method::Rcd
            }
            Some(Value::Choice(m)) if m == "markesteijn" => {
                if bayer {
                    return Err("Markesteijn needs an X-Trans pattern".into());
                }
                Method::Markesteijn
            }
            Some(Value::Choice(m)) if m == "bilinear" => Method::Bilinear,
            _ if bayer => Method::Rcd,
            _ => Method::Markesteijn,
        };
        // Optional: without a white balance, values are expected up to 1.
        let maximum = context
            .get(fields::WHITE_BALANCE)
            .and_then(|v| v.as_list())
            .filter(|l| l.len() >= 3 && l[1] > 0.0)
            .map_or(1.0, |wb| {
                [wb[0] / wb[1], 1.0, wb[2] / wb[1]]
                    .into_iter()
                    .fold(1.0f64, f64::max)
            }) as f32;
        Ok(Plan {
            cfa,
            method,
            maximum,
            scale: 1.0,
        })
    }

    fn prepare_at(&self, params: &Params, context: &Context, scale: f64) -> Result<Plan, String> {
        Ok(Plan {
            scale,
            ..self.prepare(params, context)?
        })
    }

    fn input_region(&self, plan: &Plan, output: Region, overlap: u32) -> Region {
        match plan.method {
            Method::Bilinear => output.grow(overlap),
            Method::Markesteijn => output.grow(xtrans::BORDER),
            Method::Rcd => {
                // The tile starts on an even photosite (see `rcd.rs`).
                let g = output.grow(rcd::BORDER);
                let (dx, dy) = (g.x.rem_euclid(2), g.y.rem_euclid(2));
                Region::new(
                    g.x - dx,
                    g.y - dy,
                    g.width + dx as u32,
                    g.height + dy as u32,
                )
            }
        }
    }

    fn process(
        &self,
        plan: &Plan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let input = inputs[0];
        if plan.scale < 1.0 {
            let _span = helios_core::trace::span("demosaic.direct", || {
                format!(
                    "{}x{} at {:.4} {:.2} MP",
                    output.width,
                    output.height,
                    plan.scale,
                    output.area() as f64 / 1e6
                )
            });
            let cancel = helios_core::cancel::current();
            return direct(plan, input, output, threads(), &cancel).map(|image| vec![image]);
        }
        let _span = helios_core::trace::span("demosaic", || {
            format!(
                "{:?} {}x{} {:.2} MP",
                plan.method,
                output.width,
                output.height,
                output.area() as f64 / 1e6
            )
        });
        if matches!(plan.method, Method::Rcd | Method::Markesteijn) {
            let cancel = helios_core::cancel::current();
            return self
                .in_tiles(plan, input, output, (TILE, TILE), threads(), &cancel)
                .map(|image| vec![image]);
        }
        let cfa = &plan.cfa;
        Ok(vec![Image::from_fn(output, 4, |x, y, c| {
            if c == 3 {
                return 1.0;
            }
            let c = c as usize;
            if index(cfa.color_at(x, y)) == c {
                return input.pixel(x, y)[0];
            }
            for radius in [1i64, 2] {
                let (mut sum, mut n) = (0.0f32, 0u32);
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        if index(cfa.color_at(x + dx, y + dy)) == c {
                            sum += input.pixel(x + dx, y + dy)[0];
                            n += 1;
                        }
                    }
                }
                if n > 0 {
                    return sum / n as f32;
                }
            }
            0.0
        })])
    }
}

/// The edge of the square tiles RCD and Markesteijn work in: small enough
/// for a tile's buffers to stay in the processor's caches (darktable
/// tiles them so too), large enough that the border each tile computes
/// again around it stays a small part of the work.
const TILE: u32 = 256;

/// The output over `output` at `plan.scale` straight from the mosaic
/// `input` (full resolution): each pixel the mean of each colour's
/// photosites under it, the window grown by one photosite at a time
/// (at most twice) where a colour has none; alpha 1. Rows are shared among
/// `threads` threads in bands; `cancel` is checked before each band.
fn direct(
    plan: &Plan,
    input: &Image,
    output: Region,
    threads: usize,
    cancel: &helios_core::cancel::Token,
) -> Result<Image, String> {
    let row = output.width as usize * 4;
    let mut data = vec![0.0f32; output.area() * 4];
    if row == 0 {
        return Ok(Image::new(output, 4, data));
    }
    let bounds = input.region();
    // The full-resolution columns under each output column, once.
    let columns: Vec<(i64, i64)> = (output.x..output.right())
        .map(|x| {
            let f = Region::new(x, output.y, 1, 1).full_resolution(plan.scale);
            (f.x, f.right())
        })
        .collect();
    let pixel = |x: i64, y: i64, out: &mut [f32]| {
        let f = Region::new(x, y, 1, 1).full_resolution(plan.scale);
        let (mut x0, mut x1) = columns[(x - output.x) as usize];
        let (mut y0, mut y1) = (f.y, f.bottom());
        for grow in 0..3 {
            if grow > 0 {
                (x0, x1, y0, y1) = (x0 - 1, x1 + 1, y0 - 1, y1 + 1);
            }
            let mut sum = [0.0f32; 3];
            let mut n = [0u32; 3];
            for fy in y0..y1 {
                let cy = fy.clamp(bounds.y, bounds.bottom() - 1);
                for fx in x0..x1 {
                    let cx = fx.clamp(bounds.x, bounds.right() - 1);
                    // The pattern at the photosite read (an edge one
                    // replicated keeps its own colour).
                    let c = index(plan.cfa.color_at(cx, cy));
                    sum[c] += input.pixel(cx, cy)[0];
                    n[c] += 1;
                }
            }
            if n.iter().all(|&k| k > 0) || grow == 2 {
                for c in 0..3 {
                    out[c] = if n[c] > 0 { sum[c] / n[c] as f32 } else { 0.0 };
                }
                out[3] = 1.0;
                return;
            }
        }
    };
    let rows_per_band = 16usize;
    let band = rows_per_band * row;
    let work = |first: usize, chunk: &mut [f32]| -> Result<(), String> {
        if cancel.is_cancelled() {
            return Err(helios_core::cancel::CANCELLED.to_string());
        }
        for (i, line) in chunk.chunks_mut(row).enumerate() {
            let y = output.y + (first + i) as i64;
            for (j, out) in line.chunks_mut(4).enumerate() {
                pixel(output.x + j as i64, y, out);
            }
        }
        Ok(())
    };
    let next = std::sync::atomic::AtomicUsize::new(0);
    let chunks: Vec<_> = data
        .chunks_mut(band)
        .map(|c| std::sync::Mutex::new(Some(c)))
        .collect();
    let threads = threads.clamp(1, chunks.len());
    std::thread::scope(|s| {
        let worker = || -> Result<(), String> {
            loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(slot) = chunks.get(i) else {
                    return Ok(());
                };
                let chunk = slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .expect("each band is taken once");
                work(i * rows_per_band, chunk)?;
            }
        };
        let handles: Vec<_> = (0..threads).map(|_| s.spawn(worker)).collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect::<Result<Vec<()>, String>>()
    })?;
    drop(chunks);
    Ok(Image::new(output, 4, data))
}

/// The threads a demosaic may use: the machine's.
fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

impl Demosaic {
    /// RCD or Markesteijn over `output`, in tiles of `tile` (width,
    /// height) shared among `threads` threads, each thread taking the next
    /// band of tiles not yet taken. Each tile reads its own input region,
    /// grown by the algorithm's border, so a pixel comes out the same
    /// whatever tile holds it: the tiles give the one-tile result bit for
    /// bit. `cancel` is checked before each tile.
    fn in_tiles(
        &self,
        plan: &Plan,
        input: &Image,
        output: Region,
        tile: (u32, u32),
        threads: usize,
        cancel: &helios_core::cancel::Token,
    ) -> Result<Image, String> {
        let row = output.width as usize * 4;
        let mut data = vec![0.0f32; output.area() * 4];
        let (tile_w, tile_h) = (tile.0.max(1), tile.1.max(1));
        // One band of tiles: its rows of `data`, from `y` down.
        let work = |y: u32, chunk: &mut [f32]| {
            let rows = (chunk.len() / row) as u32;
            let mut x = 0;
            while x < output.width {
                if cancel.is_cancelled() {
                    return Err(helios_core::cancel::CANCELLED.to_string());
                }
                let width = tile_w.min(output.width - x);
                let piece = Region::new(
                    output.x + i64::from(x),
                    output.y + i64::from(y),
                    width,
                    rows,
                );
                self.tile(plan, input, piece, &mut chunk[x as usize * 4..], row);
                x += width;
            }
            Ok(())
        };
        if row == 0 {
            return Ok(Image::new(output, 4, data));
        }
        let band = tile_h as usize * row;
        let bands = data.len().div_ceil(band);
        let threads = threads.clamp(1, bands);
        if threads == 1 {
            for (i, chunk) in data.chunks_mut(band).enumerate() {
                work(i as u32 * tile_h, chunk)?;
            }
        } else {
            let chunks: Vec<_> = data
                .chunks_mut(band)
                .map(|c| std::sync::Mutex::new(Some(c)))
                .collect();
            let next = std::sync::atomic::AtomicUsize::new(0);
            std::thread::scope(|s| {
                let worker = || -> Result<(), String> {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(slot) = chunks.get(i) else {
                            return Ok(());
                        };
                        let chunk = slot
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take()
                            .expect("each band is taken once");
                        work(i as u32 * tile_h, chunk)?;
                    }
                };
                let handles: Vec<_> = (0..threads).map(|_| s.spawn(worker)).collect();
                handles
                    .into_iter()
                    .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
                    .collect::<Result<Vec<()>, String>>()
            })?;
        }
        Ok(Image::new(output, 4, data))
    }

    /// One tile of RCD or Markesteijn, written to `out` as RGBA rows
    /// `stride` values apart.
    fn tile(&self, plan: &Plan, input: &Image, piece: Region, out: &mut [f32], stride: usize) {
        let r = self.input_region(plan, piece, 0);
        let at = |col: usize, row: usize| input.pixel(r.x + col as i64, r.y + row as i64)[0];
        let (dx, dy) = ((piece.x - r.x) as usize, (piece.y - r.y) as usize);
        let width = piece.width as usize;
        let rows = out.chunks_mut(stride).take(piece.height as usize);
        if plan.method == Method::Rcd {
            let (tile, planes) = rcd::demosaic(
                &plan.cfa,
                r.x,
                r.y,
                r.width as usize,
                r.height as usize,
                plan.maximum,
                at,
            );
            for (y, line) in rows.enumerate() {
                let start = (y + dy) * tile + dx;
                for (x, p) in line[..width * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .enumerate()
                {
                    let j = start + x;
                    *p = [planes[0][j], planes[1][j], planes[2][j], 1.0];
                }
            }
        } else {
            let tile = r.width as usize;
            let rgb = xtrans::demosaic(&plan.cfa, r.x, r.y, tile, r.height as usize, at);
            for (y, line) in rows.enumerate() {
                let start = (y + dy) * tile + dx;
                for (x, p) in line[..width * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .enumerate()
                {
                    let [red, green, blue] = rgb[start + x];
                    *p = [red, green, blue, 1.0];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;

    const XTRANS: &str = "GGRGGB/GGBGGR/BRGRBG/GGBGGR/GGRGGB/RBGBRG";

    fn context(cfa: &str) -> Context {
        Context::new().with(fields::CFA, Value::Choice(cfa.into()))
    }

    /// Demosaics `out` with a method, reading what it asks of `input`.
    fn run_with(cfa: &str, method: &str, input: &Image, out: Region) -> Image {
        let params = Params::new().with("method", Value::Choice(method.into()));
        let plan = Demosaic.prepare(&params, &context(cfa)).unwrap();
        let need = Demosaic.input_region(&plan, out, 2);
        Demosaic
            .process(&plan, &[&input.sample(need)], out)
            .unwrap()
            .remove(0)
    }

    fn run(cfa: &str, input: &Image, out: Region) -> Image {
        run_with(cfa, "auto", input, out)
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn a_grey_scene_stays_grey_on_both_patterns() {
        let out = Region::sized(12, 12);
        let input = Image::from_fn(out.grow(12), 1, |_, _, _| 0.4);
        for (cfa, method) in [
            ("RGGB", "rcd"),
            ("RGGB", "bilinear"),
            (XTRANS, "markesteijn"),
            (XTRANS, "bilinear"),
        ] {
            let rgb = run_with(cfa, method, &input, out);
            for p in rgb.data().as_chunks::<4>().0 {
                assert!(close(p, &[0.4, 0.4, 0.4, 1.0]), "{cfa} {method}: {p:?}");
            }
        }
    }

    #[test]
    fn a_colored_scene_comes_back_on_a_bayer_pattern() {
        // A flat scene of (0.8, 0.5, 0.2) seen through RGGB.
        let cfa = Cfa::parse("RGGB").unwrap();
        let color = [0.8f32, 0.5, 0.2];
        let out = Region::sized(8, 8);
        let input = Image::from_fn(out.grow(12), 1, |x, y, _| color[index(cfa.color_at(x, y))]);
        for method in ["rcd", "bilinear"] {
            for p in run_with("RGGB", method, &input, out)
                .data()
                .as_chunks::<4>()
                .0
            {
                assert!(close(&p[..3], &color), "{method}: {p:?}");
            }
        }
    }

    #[test]
    fn auto_is_rcd_on_bayer_and_markesteijn_on_x_trans() {
        let plan = |cfa| {
            Demosaic
                .prepare(&Params::new(), &context(cfa))
                .unwrap()
                .method
        };
        assert_eq!(plan("RGGB"), Method::Rcd);
        assert_eq!(plan(XTRANS), Method::Markesteijn);
        let rcd = Params::new().with("method", Value::Choice("rcd".into()));
        assert!(Demosaic.prepare(&rcd, &context(XTRANS)).is_err());
        let mk = Params::new().with("method", Value::Choice("markesteijn".into()));
        assert!(Demosaic.prepare(&mk, &context("RGGB")).is_err());
    }

    #[test]
    fn markesteijn_gives_the_same_pixels_whatever_the_region() {
        let scene = Image::from_fn(Region::new(-20, -20, 90, 80), 1, |x, y, _| {
            ((x * 37 + y * 11) % 23) as f32 / 23.0
                + if (x / 5 + y / 7) % 2 == 0 { 0.3 } else { 0.0 }
        });
        let whole = run(XTRANS, &scene, Region::sized(50, 40));
        for piece in [
            Region::new(3, 5, 17, 11),
            Region::new(20, 1, 9, 30),
            Region::new(31, 27, 19, 13),
        ] {
            let part = run(XTRANS, &scene, piece);
            assert_eq!(part.data(), whole.sample(piece).data(), "{piece}");
        }
    }

    #[test]
    fn markesteijn_brings_a_flat_colour_back_on_x_trans() {
        let cfa = Cfa::parse(XTRANS).unwrap();
        let color = [0.8f32, 0.5, 0.2];
        let out = Region::sized(18, 18);
        let input = Image::from_fn(out.grow(14), 1, |x, y, _| color[index(cfa.color_at(x, y))]);
        for p in run(XTRANS, &input, out).data().as_chunks::<4>().0 {
            assert!(close(&p[..3], &color), "{p:?}");
        }
    }

    #[test]
    fn rcd_gives_the_same_pixels_whatever_the_region() {
        // A detailed scene: whole and in odd-placed pieces, the same values.
        let cfa = "GBRG";
        let scene = Image::from_fn(Region::new(-20, -20, 90, 80), 1, |x, y, _| {
            ((x * 37 + y * 11) % 23) as f32 / 23.0
                + if (x / 5 + y / 7) % 2 == 0 { 0.3 } else { 0.0 }
        });
        let whole = run(cfa, &scene, Region::sized(50, 40));
        for piece in [
            Region::new(3, 5, 17, 11),
            Region::new(20, 1, 9, 30),
            Region::new(31, 27, 19, 13),
        ] {
            let part = run(cfa, &scene, piece);
            assert_eq!(part.data(), whole.sample(piece).data(), "{piece}");
        }
    }

    #[test]
    fn rcd_follows_an_edge_better_than_bilinear() {
        // A sharp vertical edge in a grey scene: RCD keeps the colours
        // flatter around it than bilinear does.
        let out = Region::sized(24, 24);
        let input = Image::from_fn(out.grow(12), 1, |x, _, _| if x < 12 { 0.1 } else { 0.8 });
        let spread = |img: &Image| {
            img.data()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| (p[0] - p[1]).abs() + (p[2] - p[1]).abs())
                .sum::<f32>()
        };
        let rcd = spread(&run_with("RGGB", "rcd", &input, out));
        let bilinear = spread(&run_with("RGGB", "bilinear", &input, out));
        assert!(rcd < bilinear, "rcd {rcd}, bilinear {bilinear}");
    }

    #[test]
    fn every_x_trans_photosite_finds_each_color_within_two() {
        let cfa = Cfa::parse(XTRANS).unwrap();
        for y in 0..6i64 {
            for x in 0..6i64 {
                for c in [Channel::Red, Channel::Green, Channel::Blue] {
                    let near = (-2..=2i64)
                        .flat_map(|dy| (-2..=2i64).map(move |dx| (dx, dy)))
                        .any(|(dx, dy)| cfa.color_at(x + dx, y + dy) == c);
                    assert!(near, "({x}, {y}) has no {c:?} within two");
                }
            }
        }
    }

    /// What `process` gave before it worked in bands: one tile, the
    /// whole input, read pixel by pixel.
    fn one_tile(plan: &Plan, input: &Image, output: Region) -> Image {
        let r = input.region();
        let at = |col: usize, row: usize| input.pixel(r.x + col as i64, r.y + row as i64)[0];
        let (w, h) = (r.width as usize, r.height as usize);
        if plan.method == Method::Rcd {
            let (stride, planes) = rcd::demosaic(&plan.cfa, r.x, r.y, w, h, plan.maximum, at);
            return Image::from_fn(output, 4, |x, y, c| {
                if c == 3 {
                    return 1.0;
                }
                planes[c as usize][(y - r.y) as usize * stride + (x - r.x) as usize]
            });
        }
        let pixels = xtrans::demosaic(&plan.cfa, r.x, r.y, w, h, at);
        Image::from_fn(output, 4, |x, y, c| {
            if c == 3 {
                return 1.0;
            }
            pixels[(y - r.y) as usize * w + (x - r.x) as usize][c as usize]
        })
    }

    #[test]
    fn tiles_give_the_one_tile_result_bit_for_bit() {
        // Odd sizes and origins, a detailed scene, a white balance above 1.
        let scene = Image::from_fn(Region::new(-40, -40, 260, 240), 1, |x, y, _| {
            ((x * 37 + y * 11) % 23) as f32 / 23.0
                + if (x / 5 + y / 7) % 2 == 0 { 0.3 } else { 0.0 }
        });
        for (cfa, out) in [
            ("GBRG", Region::new(-3, 1, 177, 151)),
            ("RGGB", Region::new(5, -7, 61, 203)),
            (XTRANS, Region::new(-1, 3, 173, 157)),
            (XTRANS, Region::new(7, -5, 59, 199)),
        ] {
            let context =
                context(cfa).with(fields::WHITE_BALANCE, Value::List(vec![2.1, 1.0, 1.6]));
            let plan = Demosaic.prepare(&Params::new(), &context).unwrap();
            let input = scene.sample(Demosaic.input_region(&plan, out, 2));
            let today = one_tile(&plan, &input, out);
            let token = helios_core::cancel::Token::new();
            for (tile, threads) in [
                ((out.width, out.height), 1),
                ((64, 64), 1),
                ((64, 64), 4),
                ((37, 23), 3),
                ((out.width, 17), 8),
                ((1000, 1000), 8),
            ] {
                let banded = Demosaic
                    .in_tiles(&plan, &input, out, tile, threads, &token)
                    .unwrap();
                assert_eq!(banded.region(), out);
                let same = banded
                    .data()
                    .iter()
                    .zip(today.data())
                    .all(|(a, b)| a.to_bits() == b.to_bits());
                assert!(
                    same,
                    "{cfa} {out} in tiles of {tile:?} on {threads} threads"
                );
            }
            let processed = Demosaic.process(&plan, &[&input], out).unwrap().remove(0);
            assert_eq!(processed.data(), today.data(), "{cfa} {out}");
        }
    }

    #[test]
    fn a_cancelled_render_gives_up() {
        let out = Region::sized(80, 300);
        let input = Image::from_fn(out.grow(14), 1, |_, _, _| 0.4);
        let token = helios_core::cancel::Token::new();
        token.cancel();
        for cfa in ["RGGB", XTRANS] {
            let plan = Demosaic.prepare(&Params::new(), &context(cfa)).unwrap();
            let need = Demosaic.input_region(&plan, out, 2);
            let input = input.sample(need);
            let err = helios_core::cancel::scope(&token, || {
                Demosaic.process(&plan, &[&input], out).unwrap_err()
            });
            assert_eq!(err, helios_core::cancel::CANCELLED, "{cfa}");
            assert!(Demosaic.process(&plan, &[&input], out).is_ok(), "{cfa}");
        }
    }

    #[test]
    fn a_missing_pattern_is_named() {
        let err = Demosaic
            .prepare(&Params::new(), &Context::new())
            .unwrap_err();
        assert!(err.contains("photo.cfa"), "{err}");
    }

    #[test]
    fn it_keeps_the_module_contract_on_both_patterns() {
        helios_testkit::assert_contract_in(Demosaic, &context("RGGB"));
        helios_testkit::assert_contract_in(Demosaic, &context(XTRANS));
    }
}
