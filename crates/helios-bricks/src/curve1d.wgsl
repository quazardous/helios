// core.curve1d@1 on the GPU: the same formula as the CPU form, a tone
// curve on luminance as a ratio, then a chroma curve over luminance, then
// the gamut compressed toward the luminance, the colour channels clamped
// to [0, 1]; the fourth kept.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// [0]: the knot count (x), 1 with a chroma curve (y), 1 with either curve
// (z); [1..3]: the 8 chroma ratios; then two knots per vec4 (scene,
// display, scene, display), 64 knots at most.
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 35>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes.
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

const LUMA: vec3<f32> = vec3<f32>(0.2627002, 0.6779981, 0.0593017);
const CHROMA_NODES: u32 = 8u;
// The CPU form's KNEE: the share of the room between a pixel's luminance
// and 1 its brightest channel takes before the compression starts.
const KNEE: f32 = 0.8;

fn luma(p: vec3<f32>) -> f32 {
    return LUMA.x * p.x + LUMA.y * p.y + LUMA.z * p.z;
}

fn knot(i: u32) -> vec2<f32> {
    let v = plan[3u + i / 2u];
    if (i % 2u == 0u) {
        return v.xy;
    }
    return v.zw;
}

fn ratio(i: u32) -> f32 {
    let v = plan[1u + i / 4u];
    return v[i % 4u];
}

fn tone(s: f32, n: u32) -> f32 {
    if (!(s > 0.0)) {
        return 0.0;
    }
    let last = knot(n - 1u);
    if (s >= last.x) {
        let before = knot(n - 2u);
        let slope = (last.y - before.y) / (last.x - before.x);
        let room = 1.0 - last.y;
        if (!(room > 0.0) || !(slope > 0.0)) {
            return last.y;
        }
        return last.y + room * (1.0 - exp(-slope * (s - last.x) / room));
    }
    // The first knot whose scene is above s (the CPU's partition point).
    var i = 1u;
    for (var k = 1u; k < n; k = k + 1u) {
        if (knot(k).x <= s) {
            i = k + 1u;
        }
    }
    let a = knot(i - 1u);
    let b = knot(i);
    return a.y + (b.y - a.y) * (s - a.x) / (b.x - a.x);
}

fn chroma(d: f32) -> f32 {
    let x = clamp(d, 0.0, 1.0) * f32(CHROMA_NODES - 1u);
    let i = min(u32(floor(x)), CHROMA_NODES - 2u);
    let f = x - f32(i);
    return ratio(i) * (1.0 - f) + ratio(i + 1u) * f;
}

// The pixel desaturated toward its luminance until its brightest channel
// is under 1, softly from KNEE on; unchanged within the knee or at white.
fn compressed(p: vec3<f32>) -> vec3<f32> {
    let d = luma(p);
    let room = 1.0 - d;
    if (!(room > 0.0)) {
        return p;
    }
    let x = (max(max(p.x, p.y), p.z) - d) / room;
    if (!(x - KNEE > 0.0)) {
        return p;
    }
    let y = (x - KNEE) / (1.0 - KNEE);
    let g = KNEE + (1.0 - KNEE) * y / (1.0 + y);
    return vec3<f32>(d) + (g / x) * (p - vec3<f32>(d));
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let base = p * 4u;
    let rgb = vec3<f32>(input[base], input[base + 1u], input[base + 2u]);
    var o = rgb;
    if (plan[0].z > 0.0) {
        let n = u32(plan[0].x);
        if (n > 0u) {
            let s = luma(rgb);
            if (s > 0.0) {
                o = rgb * (tone(s, n) / s);
            } else {
                o = vec3<f32>(0.0);
            }
        }
        if (plan[0].y > 0.0) {
            let d = luma(o);
            o = vec3<f32>(d) + chroma(d) * (o - vec3<f32>(d));
        }
        o = clamp(compressed(o), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    output[base] = o.x;
    output[base + 1u] = o.y;
    output[base + 2u] = o.z;
    output[base + 3u] = input[base + 3u];
}
