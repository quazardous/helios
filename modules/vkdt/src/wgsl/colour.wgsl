
// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/colour/main-impl.glsl. vkdt: copyright 2019 johannes
// hanika, under the BSD-2-Clause licence; its notice is in
// modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's colour: the camera matrix and the CAT16 balance (one matrix, from
// the plan), the highlight clip, exposure, then the saturation in dt UCS
// (ucs.wgsl, put in front of this file). Alpha set to 1.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// [0..3]: the matrix's rows; [3]: clip value, clip on (1), gain, sat.
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 4>;
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let base = p * 4u;
    let cam = vec3<f32>(input[base], input[base + 1u], input[base + 2u]);
    var rgb = vec3<f32>(dot(plan[0].xyz, cam), dot(plan[1].xyz, cam), dot(plan[2].xyz, cam));
    if (plan[3].y > 0.5) {
        rgb = min(rgb, vec3<f32>(plan[3].x));
    }
    rgb = rgb * plan[3].z;
    if (plan[3].w != 1.0) {
        rgb = max(rgb, vec3<f32>(0.0));
        if (rgb.x + rgb.y + rgb.z > 0.0) {
            var jch = xyy_to_dt_ucs_jch(rec2020_to_xyy(rgb), 1.0);
            jch.y = clamp(jch.y * plan[3].w, 0.0, 1.0);
            rgb = xyy_to_rec2020(dt_ucs_jch_to_xyy(jch, 1.0));
        }
    }
    rgb = clamp(rgb, vec3<f32>(-65535.0), vec3<f32>(65535.0));
    output[base] = rgb.x;
    output[base + 1u] = rgb.y;
    output[base + 2u] = rgb.z;
    output[base + 3u] = 1.0;
}
