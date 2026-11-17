// Ported from vkdt (https://github.com/hanatos/vkdt, commit afc3425):
// src/pipe/modules/llap/curve.comp, src/pipe/modules/llap/llap.glsl,
// src/pipe/modules/shared.glsl. vkdt: copyright 2019 johannes hanika, under
// the BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's llap curve (llap/curve.comp): the luminance through the ten tone
// curves, one per grey level i / 9, then the luminance itself: eleven
// channels. plan[0]: sigma, shadows, highlights, clarity.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes.
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

fn curve(x: f32, g: f32) -> f32 {
    let sigma = plan[0].x;
    let c = x - g;
    var ssigma = -sigma;
    var shadhi = plan[0].z;
    if (c > 0.0) {
        ssigma = sigma;
        shadhi = plan[0].y;
    }
    var val = 0.0;
    if (abs(c) > 2.0 * sigma) {
        val = g + ssigma + shadhi * (c - ssigma);
    } else {
        let t = clamp(c / (2.0 * ssigma), 0.0, 1.0);
        let t2 = t * t;
        let mt = 1.0 - t;
        val = g + ssigma * 2.0 * mt * t + t2 * (ssigma + ssigma * shadhi);
    }
    val = val + plan[0].w * c * exp(-c * c / (2.0 * sigma * sigma / 3.0));
    return val;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    // The input covers the output exactly.
    let i = p * 4u;
    let rgb = clamp(
        vec3<f32>(input[i], input[i + 1u], input[i + 2u]),
        vec3<f32>(-1000.0),
        vec3<f32>(1000.0),
    );
    let y = 0.26270021 * rgb.x + 0.677998072 * rgb.y + 0.0593017165 * rgb.z;
    let o = p * 11u;
    for (var k = 0u; k < 10u; k = k + 1u) {
        output[o + k] = curve(y, f32(k) / 9.0);
    }
    output[o + 10u] = y;
}
