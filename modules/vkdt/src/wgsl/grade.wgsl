// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/grade/main.comp. vkdt: copyright 2019 johannes hanika,
// under the BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause

// vkdt's grade: lift, gamma, gain and offset, on all tones alike or by
// zone (vkdt's main.comp). Alpha set to 1.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// [0]: lift rgb, mode (1: zoned log); [1]: gamma rgb, shadow pivot;
// [2]: gain rgb, highlight pivot; [3]: offset rgb.
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 4>;
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

// GLSL's smoothstep; with e1 <= e0, a step at e0 (as the CPU reference).
fn smooth_step(e0: f32, e1: f32, x: f32) -> f32 {
    if (e1 <= e0) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let base = p * 4u;
    let rgb = vec3<f32>(input[base], input[base + 1u], input[base + 2u]);
    var lift = plan[0].xyz;
    var gamma = plan[1].xyz;
    var gain = plan[2].xyz;
    if (plan[0].w > 0.5) {
        let m = max(rgb, vec3<f32>(0.0));
        var l = 0.2126 * m.x + 0.7152 * m.y + 0.0722 * m.z;
        l = clamp(0.67 + log2(max(l, 1e-6)) * 0.11, 0.0, 1.0);
        let w_s = 1.0 - smooth_step(0.0, plan[1].w, l);
        let w_h = smooth_step(plan[2].w, 1.0, l);
        let w_m = 1.0 - w_s - w_h;
        gain = vec3<f32>(1.0) + (gain - vec3<f32>(1.0)) * w_h;
        lift = lift * w_s;
        gamma = vec3<f32>(1.0) + (gamma - vec3<f32>(1.0)) * w_m;
    }
    var v = gain * rgb;
    v = v * (vec3<f32>(1.0) - lift) + lift;
    v = pow(max(v, vec3<f32>(0.0)), vec3<f32>(1.0) / gamma) + plan[3].xyz;
    output[base] = v.x;
    output[base + 1u] = v.y;
    output[base + 2u] = v.z;
    output[base + 3u] = 1.0;
}
