// vkdt's resample (shared/resample.comp, sample_catmull_rom in
// shared.glsl): upsampling by plan[0].x (a power of two) with a
// Catmull-Rom kernel over 4x4 input pixels.

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

// The second of the four input pixels output pixel o reads.
fn first(o: i32, f: i32) -> i32 {
    let n = 2 * o + 1 - f;
    let d = 2 * f;
    // Floor division.
    return select(n / d, (n - d + 1) / d, n < 0);
}

fn weights(o: i32, f: i32, t: i32) -> vec4<f32> {
    let s = (f32(o) + 0.5) / f32(f) - 0.5 - f32(t);
    return vec4<f32>(
        s * (-0.5 + s * (1.0 - 0.5 * s)),
        1.0 + s * s * (-2.5 + 1.5 * s),
        s * (0.5 + s * (2.0 - 1.5 * s)),
        s * s * (-0.5 + 0.5 * s),
    );
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
    let f = i32(plan[0].x);
    let tx = first(x, f);
    let ty = first(y, f);
    let wx = weights(x, f, tx);
    let wy = weights(y, f, ty);
    for (var c = 0u; c < 2u; c = c + 1u) {
        var s = 0.0;
        for (var j = 0; j < 4; j = j + 1) {
            for (var i = 0; i < 4; i = i + 1) {
                s = s + wx[i] * wy[j] * texel(tx + i - 1, ty + j - 1, c);
            }
        }
        output[p * 2u + c] = s;
    }
}
