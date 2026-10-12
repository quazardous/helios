// vkdt's exposure: rgb * 2^ev, alpha set to 1 (vkdt's main.comp).

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// x: the gain, 2^ev.
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let base = p * 4u;
    for (var c = 0u; c < 3u; c = c + 1u) {
        output[base + c] = input[base + c] * plan[0].x;
    }
    output[base + 3u] = 1.0;
}
