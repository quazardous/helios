// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/shared/blurs.comp, src/pipe/modules/shared.glsl. vkdt:
// copyright 2019 johannes hanika, under the BSD-2-Clause licence; its notice is
// in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's blurs (shared/blurs.comp): four bilinear taps over a 3x3
// support, plan[0].x from the centre on each axis.

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
    let x = f32(bitcast<i32>(shape[2].x) + i32(p % ow));
    let y = f32(bitcast<i32>(shape[2].y) + i32(p / ow));
    let o = plan[0].x;
    for (var c = 0u; c < 2u; c = c + 1u) {
        output[p * 2u + c] = (bilinear(x - o, y - o, c) + bilinear(x - o, y + o, c)
            + bilinear(x + o, y - o, c) + bilinear(x + o, y + o, c)) / 4.0;
    }
}
