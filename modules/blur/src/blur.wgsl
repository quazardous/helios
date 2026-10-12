// The blur's GPU variant: the same Gaussian as the CPU code, in one 2D pass
// (weights w[|dx|] * w[|dy|]); sums run in another order than the CPU's two
// passes, so results match within float rounding, not bit for bit.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// [0].x: the reach in pixels; from [1] on: weights from the centre outward.
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 64>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes.
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

fn weight(i: i32) -> f32 {
    let k = u32(abs(i));
    return plan[1u + k / 4u][k % 4u];
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let in_w = i32(shape[1].x);
    let out_w = shape[1].z;
    let off_x = (shape[1].x - shape[1].z) / 2u;
    let off_y = (shape[1].y - shape[1].w) / 2u;
    let x = i32(p % out_w + off_x);
    let y = i32(p / out_w + off_y);
    let reach = i32(plan[0].x);
    var sum = vec4<f32>(0.0);
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        let wy = weight(dy);
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            let i = u32(((y + dy) * in_w + x + dx) * 4);
            let v = vec4<f32>(input[i], input[i + 1u], input[i + 2u], input[i + 3u]);
            sum = sum + v * (wy * weight(dx));
        }
    }
    let o = p * 4u;
    output[o] = sum.x;
    output[o + 1u] = sum.y;
    output[o + 2u] = sum.z;
    output[o + 3u] = sum.w;
}
