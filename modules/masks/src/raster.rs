//! A raster mask member: another module's mask reused, darktable's raster
//! masks (`dt_dev_get_raster_mask`, the raster branch of
//! `dt_develop_blend_process` in `src/develop/blend.c`).

use helios_core::{
    Context, Family, Image, Manifest, Module, ModuleKind, Needs, ParamHint, ParamKind, ParamSpec,
    Params, Region, Role,
};

use crate::{flag, mask_port};

/// Reads another node's one-channel `mask` output and hands it to a group's
/// port: `(invert ? 1 - m : m) * opacity`, darktable's raster member.
///
/// The engine asks the source for the region and scale this node is
/// rendered at, so the mask reused is the one the source computes there:
/// tiles and previews need nothing more. Left unconnected (its source
/// removed), it gives 0, darktable's fallback when the raster mask is
/// missing, unlike an empty group which gives 1.
pub struct MaskRaster;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterPlan {
    pub invert: bool,
    pub opacity: f32,
}

impl Module for MaskRaster {
    type Plan = RasterPlan;

    fn manifest(&self) -> Manifest {
        Manifest {
            name: "mask_raster".into(),
            version: 1,
            description: "Reuses another module's mask as a member: inversion, then opacity".into(),
            params: vec![
                ParamSpec {
                    name: "invert".into(),
                    description: "invert the reused mask, before its opacity".into(),
                    kind: ParamKind::Bool { default: false },
                },
                ParamSpec::float(
                    "opacity",
                    "multiplies the reused mask, after its inversion",
                    0.0,
                    1.0,
                    1.0,
                ),
            ],
            inputs: vec![mask_port("mask").optional()],
            outputs: vec![mask_port("mask")],
            context: vec![],
            needs: Needs::default(),
            kind: ModuleKind::Process,
            family: Family::Mask,
            role: Role::General,
        }
    }

    fn param_hints(&self) -> Vec<ParamHint> {
        vec![ParamHint::new("opacity").unit("%").display(100.0, 0)]
    }

    fn prepare(&self, params: &Params, _: &Context) -> Result<RasterPlan, String> {
        Ok(RasterPlan {
            invert: flag(params, "invert"),
            opacity: params.float("opacity") as f32,
        })
    }

    fn process(
        &self,
        plan: &RasterPlan,
        inputs: &[&Image],
        output: Region,
    ) -> Result<Vec<Image>, String> {
        let source = inputs[0];
        if source.is_absent() {
            return Ok(vec![Image::new(output, 1, vec![0.0; output.area()])]);
        }
        if !plan.invert && plan.opacity == 1.0 {
            return Ok(vec![source.sample(output)]);
        }
        let data = source
            .sample(output)
            .data()
            .iter()
            .map(|&m| (if plan.invert { 1.0 - m } else { m }) * plan.opacity)
            .collect();
        Ok(vec![Image::new(output, 1, data)])
    }
}
