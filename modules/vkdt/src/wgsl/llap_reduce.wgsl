// Ported from vkdt (https://github.com/hanatos/vkdt, commit afc3425):
// src/pipe/modules/llap/reduce.comp, src/pipe/modules/shared.glsl
// (sample_semisoft) and the sampler of src/qvk/qvk.c (linear, mirrored
// repeat). vkdt: copyright 2019 johannes hanika, under the BSD-2-Clause
// licence; its notice is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's llap reduce (llap/reduce.comp): a pyramid level at half the
// resolution, eleven channels, each through sample_semisoft centred on
// input pixel 2 o. plan[0]: the finer level's domain (x, y, width,
// height), lookups mirrored into it.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes; [2]: output and input origins (i32).
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 3>;

// Vulkan's mirrored repeat of i into 0..n.
fn mirror(i: i32, n: i32) -> i32 {
    let p = 2 * n;
    let m = ((i % p) + p) % p;
    if (m < n) {
        return m;
    }
    return p - 1 - m;
}

// Channel c at relative pixel (x, y) of the domain, mirrored.
fn texel(x: i32, y: i32, c: u32) -> f32 {
    let ax = i32(plan[0].x) + mirror(x, i32(plan[0].z)) - bitcast<i32>(shape[2].z);
    let ay = i32(plan[0].y) + mirror(y, i32(plan[0].w)) - bitcast<i32>(shape[2].w);
    return input[(u32(ay) * shape[1].x + u32(ax)) * 11u + c];
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
    let x = bitcast<i32>(shape[2].x) + i32(p % ow);
    let y = bitcast<i32>(shape[2].y) + i32(p / ow);
    let cx = 2.0 * f32(x);
    let cy = 2.0 * f32(y);
    for (var c = 0u; c < 11u; c = c + 1u) {
        let s = bilinear(cx - 0.5, cy - 0.5, c) + bilinear(cx + 0.5, cy - 0.5, c)
            + bilinear(cx - 0.5, cy + 0.5, c) + bilinear(cx + 0.5, cy + 0.5, c);
        output[p * 11u + c] = s / 4.0;
    }
}
