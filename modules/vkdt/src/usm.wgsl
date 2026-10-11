// vkdt's usm: a "flower" blur of five bilinear taps, then the difference to
// the centre boosted by `amount` where it is under `thrs` (vkdt's main.comp
// and sample_flower4 in shared.glsl).

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// x: amount, y: thrs.
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1>;
// [0]: pixels, input channels, output channels; [1]: input and output sizes.
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

fn texel(x: i32, y: i32, c: u32) -> f32 {
    let w = i32(shape[1].x);
    let h = i32(shape[1].y);
    let cx = clamp(x, 0, w - 1);
    let cy = clamp(y, 0, h - 1);
    return input[(u32(cy) * u32(w) + u32(cx)) * 4u + c];
}

// Bilinear at a position in texel centres of the input.
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
    let p = id.x;
    if (p >= shape[0].x) {
        return;
    }
    let ow = shape[1].z;
    let margin_x = f32((shape[1].x - shape[1].z) / 2u);
    let margin_y = f32((shape[1].y - shape[1].w) / 2u);
    let x = f32(p % ow) + margin_x;
    let y = f32(p / ow) + margin_y;
    let t = 36.0 / 256.0;
    let side = (1.0 - t) / 4.0;
    for (var c = 0u; c < 3u; c = c + 1u) {
        let centre = texel(i32(x), i32(y), c);
        let blur = t * centre
            + side * bilinear(x + 1.2, y + 0.4, c)
            + side * bilinear(x - 1.2, y - 0.4, c)
            + side * bilinear(x - 0.4, y + 1.2, c)
            + side * bilinear(x + 0.4, y - 1.2, c);
        let d = centre - blur;
        let gain = select(1.0, plan[0].x, d < plan[0].y);
        output[p * 4u + c] = blur + gain * d;
    }
    output[p * 4u + 3u] = 1.0;
}
