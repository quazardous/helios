//! demosaic: the sensor mosaic (one value per photosite, behind a color
//! filter) to camera RGB, three values per pixel.
//!
//! First method: **bilinear**, for any filter pattern. Each missing color of
//! a photosite is the mean of the nearest photosites of that color: those of
//! the 3 x 3 neighbourhood, else of the 5 x 5 one (some X-Trans positions
//! lack a red or a blue nearby). On a Bayer pattern this is darktable's
//! bilinear fallback. The pattern comes from the camera's description, a
//! declared context input, anchored at the image origin (the loader shifts
//! it to its crop).
//!
//! Next: darktable's defaults (RCD for Bayer, Markesteijn for X-Trans),
//! once parity can be measured against darktable.
//!
//! Known limit: within two pixels of the image border the framework
//! replicates edge photosites, whose filter color is not the one the
//! pattern gives there; those pixels are approximate.

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, Params, PortSpec, Region, Role,
};
use helios_photo::Cfa;
use helios_photo::camera::fields;
use helios_photo::cfa::Channel;

pub struct Demosaic;

fn index(c: Channel) -> usize {
    match c {
        Channel::Red => 0,
        Channel::Green => 1,
        Channel::Blue => 2,
    }
}

impl Module for Demosaic {
    type Plan = Cfa;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "demosaic".into(),
            version: 1,
            description: "Turn the sensor mosaic into camera RGB (bilinear)".into(),
            params: vec![],
            inputs: vec![PortSpec::image("input", 1)],
            outputs: vec![PortSpec::image("output", 4)],
            context: vec![fields::CFA.into()],
            needs: Needs { overlap: 2 },
            kind: ModuleKind::Process,
            family: Family::Detail,
            role: Role::General,
        }
    }

    fn prepare(&self, _: &Params, context: &Context) -> Result<Cfa, String> {
        let name = context
            .get(fields::CFA)
            .and_then(|v| v.as_choice())
            .ok_or_else(|| format!("context field `{}`: a filter pattern needed", fields::CFA))?;
        Cfa::parse(name)
    }

    fn process(&self, cfa: &Cfa, inputs: &[&Image], output: Region) -> Result<Vec<Image>, String> {
        let input = inputs[0];
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

#[cfg(test)]
mod tests {
    use super::*;
    use helios_core::Value;

    const XTRANS: &str = "GGRGGB/GGBGGR/BRGRBG/GGBGGR/GGRGGB/RBGBRG";

    fn context(cfa: &str) -> Context {
        Context::new().with(fields::CFA, Value::Choice(cfa.into()))
    }

    fn run(cfa: &str, input: &Image, out: Region) -> Image {
        let plan = Demosaic.prepare(&Params::new(), &context(cfa)).unwrap();
        Demosaic.process(&plan, &[input], out).unwrap().remove(0)
    }

    #[test]
    fn a_grey_scene_stays_grey_on_both_patterns() {
        let out = Region::sized(12, 12);
        let input = Image::from_fn(out.grow(2), 1, |_, _, _| 0.4);
        for cfa in ["RGGB", XTRANS] {
            let rgb = run(cfa, &input, out);
            for p in rgb.data().as_chunks::<4>().0 {
                assert_eq!(*p, [0.4, 0.4, 0.4, 1.0], "{cfa}");
            }
        }
    }

    #[test]
    fn a_colored_scene_comes_back_on_a_bayer_pattern() {
        // A flat scene of (0.8, 0.5, 0.2) seen through RGGB.
        let cfa = Cfa::parse("RGGB").unwrap();
        let color = [0.8f32, 0.5, 0.2];
        let out = Region::sized(8, 8);
        let input = Image::from_fn(out.grow(2), 1, |x, y, _| color[index(cfa.color_at(x, y))]);
        for p in run("RGGB", &input, out).data().as_chunks::<4>().0 {
            assert_eq!(&p[..3], &color);
        }
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
