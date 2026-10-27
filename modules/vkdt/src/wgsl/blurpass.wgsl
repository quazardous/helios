// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/shared/blurh.comp, src/pipe/modules/shared/blurv.comp,
// src/pipe/modules/shared/blur_head.glsl. vkdt: copyright 2019 johannes hanika,
// under the BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's blurh and blurv (shared/blurh.comp, blurv.comp): one pass of a
// separable Gaussian. plan[0].x: 1 along columns, 0 along rows; plan[0].y:
// taps on each side; then the weights, the centre's first, four a vec4.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 17>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes; [2]: output and input origins (i32).
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 3>;

fn texel(x: i32, y: i32, c: u32) -> f32 {
    let ix = clamp(x - bitcast<i32>(shape[2].z), 0, i32(shape[1].x) - 1);
    let iy = clamp(y - bitcast<i32>(shape[2].w), 0, i32(shape[1].y) - 1);
    return input[(u32(iy) * shape[1].x + u32(ix)) * 2u + c];
}

// Weight k (0 the centre), from index 2 of the plan on.
fn weight(k: u32) -> f32 {
    let i = k + 2u;
    return plan[i / 4u][i % 4u];
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let ow = shape[1].z;
    let x = bitcast<i32>(shape[2].x) + i32(p % ow);
    let y = bitcast<i32>(shape[2].y) + i32(p / ow);
    var dx = 1;
    var dy = 0;
    if (plan[0].x > 0.5) {
        dx = 0;
        dy = 1;
    }
    let taps = u32(plan[0].y);
    var total = weight(0u);
    for (var k = 1u; k <= taps; k = k + 1u) {
        total = total + 2.0 * weight(k);
    }
    for (var c = 0u; c < 2u; c = c + 1u) {
        var s = weight(0u) * texel(x, y, c);
        for (var k = 1u; k <= taps; k = k + 1u) {
            let o = i32(k);
            s = s + weight(k) * (texel(x + o * dx, y + o * dy, c) + texel(x - o * dx, y - o * dy, c));
        }
        output[p * 2u + c] = s / total;
    }
}
