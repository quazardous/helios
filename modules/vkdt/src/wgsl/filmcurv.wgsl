// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/filmcurv/main.comp, src/pipe/modules/filmcurv/params.glsl,
// src/pipe/modules/shared.glsl (adjust_colour_dng). vkdt: copyright 2019
// johannes hanika, under the BSD-2-Clause licence; its notice is in
// modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's filmcurv: the Weibull curve on each channel, then (hsl mode) the
// DNG hue-preserving step. Alpha set to 1.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// x: 1 / lambda, y: k, z: bias, w: 1 for hsl, 0 for per channel.
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

fn weibull_cdf(x: vec3<f32>, il: f32, k: f32) -> vec3<f32> {
    return vec3<f32>(1.0) - exp(-pow(max(x, vec3<f32>(1e-7)) * il, vec3<f32>(k)));
}

fn adjust_colour_dng(c0_in: vec3<f32>, c1_in: vec3<f32>) -> vec3<f32> {
    var c0 = c0_in;
    var c1 = c1_in;
    let fx = c0.z > c0.y;
    if (fx) {
        c0 = c0.xzy;
        c1 = c1.xzy;
    }
    let fy = c0.y > c0.x;
    if (fy) {
        c0 = c0.yxz;
        c1 = c1.yxz;
    }
    let fz = c0.z > c0.y;
    if (fz) {
        c0 = c0.xzy;
        c1 = c1.xzy;
    }
    let t = (c0.y - c0.z + 1e-6) / (c0.x - c0.z + 1e-6);
    c1.y = c1.z * (1.0 - t) + c1.x * t;
    if (fz) {
        c1 = c1.xzy;
    }
    if (fy) {
        c1 = c1.yxz;
    }
    if (fx) {
        c1 = c1.xzy;
    }
    return c1;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let base = p * 4u;
    let col0 = vec3<f32>(input[base], input[base + 1u], input[base + 2u]) + vec3<f32>(plan[0].z);
    var col1 = weibull_cdf(col0, plan[0].x, plan[0].y);
    if (plan[0].w > 0.5) {
        col1 = adjust_colour_dng(col0, col1);
    }
    output[base] = col1.x;
    output[base + 1u] = col1.y;
    output[base + 2u] = col1.z;
    output[base + 3u] = 1.0;
}
