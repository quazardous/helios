// The mask display on the GPU: the same composite as the CPU reference
// (`mask_display::composite`), laid over a result that stayed on the
// device. Run by `helios_gpu::Gpu::composite`, one invocation a pixel.
//
// The output is RGBA as the viewport's host tiles are: the image's first
// three channels (a grey spread over them), an alpha of 1.

@group(0) @binding(0) var<storage, read> pixels: array<f32>;
@group(0) @binding(1) var<storage, read> masks: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;
// [0..3]: working space to linear sRGB, by rows; [3..6]: back, by rows;
// [6]: the overlay's colour (encoded sRGB) and opacity; [7]: the
// desaturation, then the luminance weights.
@group(0) @binding(3) var<uniform> plan: array<vec4<f32>, 8>;
// [0]: pixels, image channels, mask channels, invocations per grid row;
// [1]: x the mode (0 overlay, 1 grey).
@group(0) @binding(4) var<uniform> shape: array<vec4<u32>, 2>;

fn encode(v: f32) -> f32 {
    if (v <= 0.0031308) {
        return 12.92 * v;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn decode(v: f32) -> f32 {
    if (v <= 0.04045) {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn apply(row: u32, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        dot(plan[row].xyz, v),
        dot(plan[row + 1u].xyz, v),
        dot(plan[row + 2u].xyz, v),
    );
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let n = shape[0].y;
    let i = p * n;
    let rgb = vec3<f32>(pixels[i], pixels[i + min(1u, n - 1u)], pixels[i + min(2u, n - 1u)]);
    let m = clamp(masks[p * shape[0].z], 0.0, 1.0);
    var shown: vec3<f32>;
    if (shape[1].x == 1u) {
        shown = vec3<f32>(decode(m));
    } else {
        let y = dot(plan[7].yzw, rgb);
        let base = rgb + (vec3<f32>(y) - rgb) * plan[7].x;
        let lin = clamp(apply(0u, base), vec3<f32>(0.0), vec3<f32>(1.0));
        let e = vec3<f32>(encode(lin.x), encode(lin.y), encode(lin.z));
        let a = m * plan[6].w;
        let mixed = e + (plan[6].xyz - e) * a;
        shown = apply(3u, vec3<f32>(decode(mixed.x), decode(mixed.y), decode(mixed.z)));
    }
    let o = p * 4u;
    output[o] = shown.x;
    output[o + 1u] = shown.y;
    output[o + 2u] = shown.z;
    output[o + 3u] = 1.0;
}
