// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/shared/blur.comp, src/pipe/modules/shared.glsl. vkdt:
// copyright 2019 johannes hanika, under the BSD-2-Clause licence; its notice is
// in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's blur (shared/blur.comp): the 5x5 a-trous kernel over two
// channels, sampled bilinearly; at half the resolution when plan[0].x is 1
// (output pixel o centred at input position 2 o + 0.5).

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes; [2]: output and input origins (i32).
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 3>;

fn texel(x: i32, y: i32, c: u32) -> f32 {
    let ix = clamp(x - bitcast<i32>(shape[2].z), 0, i32(shape[1].x) - 1);
    let iy = clamp(y - bitcast<i32>(shape[2].w), 0, i32(shape[1].y) - 1);
    return input[(u32(iy) * shape[1].x + u32(ix)) * 2u + c];
}

// Bilinear at a position in pixel centres (image coordinates).
fn bilinear(px: f32, py: f32, c: u32) -> f32 {
    let x0 = floor(px);
    let y0 = floor(py);
    let fx = px - x0;
    let fy = py - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let top = mix(texel(ix, iy, c), texel(ix + 1, iy, c), fx);
    let bottom = mix(texel(ix, iy + 1, c), texel(ix + 1, iy + 1, c), fx);
    return mix(top, bottom, fy);
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
    var cx = f32(x);
    var cy = f32(y);
    if (plan[0].x > 0.5) {
        cx = 2.0 * f32(x) + 0.5;
        cy = 2.0 * f32(y) + 0.5;
    }
    let w = array<f32, 5>(1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0);
    var s0 = 0.0;
    var s1 = 0.0;
    for (var j = 0; j < 5; j = j + 1) {
        for (var i = 0; i < 5; i = i + 1) {
            let px = cx + f32(i) - 2.0;
            let py = cy + f32(j) - 2.0;
            let k = w[i] * w[j];
            s0 = s0 + k * bilinear(px, py, 0u);
            s1 = s1 + k * bilinear(px, py, 1u);
        }
    }
    output[p * 2u] = s0;
    output[p * 2u + 1u] = s1;
}
