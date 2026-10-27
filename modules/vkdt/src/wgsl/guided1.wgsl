// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/shared/guided1.comp, src/pipe/modules/shared.glsl. vkdt:
// copyright 2019 johannes hanika, under the BSD-2-Clause licence; its notice is
// in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's guided1 (shared/guided1.comp): luminance I and I*I clamped to 10.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes.
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    _ = plan[0];
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    // The input covers the output exactly.
    let i = p * 4u;
    let l = 0.26270021 * input[i] + 0.677998072 * input[i + 1u] + 0.0593017165 * input[i + 2u];
    output[p * 2u] = l;
    output[p * 2u + 1u] = clamp(l * l, 0.0, 10.0);
}
