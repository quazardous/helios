// Exposure's GPU variant: the same formula as the CPU code,
// out = (in - black) * scale on the color channels, the fourth kept.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// x: black, y: scale.
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
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
    for (var c = 0u; c < 3u; c = c + 1u) {
        output[base + c] = (input[base + c] - plan[0].x) * plan[0].y;
    }
    output[base + 3u] = input[base + 3u];
}
