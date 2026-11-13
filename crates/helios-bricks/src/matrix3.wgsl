// core.matrix3@1 on the GPU: the same formula as the CPU form,
// out = M · in on the colour channels, the fourth kept.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// One row of M per vec4 (x, y, z; w unused).
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 3>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes.
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let base = p * 4u;
    let r = input[base];
    let g = input[base + 1u];
    let b = input[base + 2u];
    for (var c = 0u; c < 3u; c = c + 1u) {
        output[base + c] = plan[c].x * r + plan[c].y * g + plan[c].z * b;
    }
    output[base + 3u] = input[base + 3u];
}
