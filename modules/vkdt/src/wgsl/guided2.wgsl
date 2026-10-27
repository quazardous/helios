// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/shared/guided2.comp. vkdt: copyright 2019 johannes hanika,
// under the BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's guided2 (shared/guided2.comp): the linear model (a, b) from the
// blurred (I, I*I). plan[0].x: edges^2 / 1000.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
// [0]: pixels, input channels, output channels, invocations per grid row.
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    // The input covers the output exactly.
    let mean = input[p * 2u];
    let corr = input[p * 2u + 1u];
    let v = corr - mean * mean;
    let a = v / (v + plan[0].x);
    output[p * 2u] = a;
    output[p * 2u + 1u] = mean - a * mean;
}
