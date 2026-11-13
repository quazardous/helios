// core.huesat@1 on the GPU: the same formula as the CPU form,
// out = T(M · in) on the colour channels, the fourth kept.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// [0..3]: the rows of M (x, y, z; w unused); [3]: the neutral (x, y, z)
// and 1 when there is a table (w); then two nodes per vec4 (hue shift,
// saturation scale, hue shift, saturation scale).
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 904>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes.
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

const HUES: u32 = 72u;
const SATURATIONS: u32 = 5u;
const VALUES: u32 = 5u;
const LUMA: vec3<f32> = vec3<f32>(0.2627002, 0.6779981, 0.0593017);

fn wrap(h: f32) -> f32 {
    return h - 360.0 * floor(h / 360.0);
}

fn luma(p: vec3<f32>) -> f32 {
    return LUMA.x * p.x + LUMA.y * p.y + LUMA.z * p.z;
}

fn hsv(p: vec3<f32>) -> vec3<f32> {
    let mx = max(max(p.x, p.y), p.z);
    let mn = min(min(p.x, p.y), p.z);
    let c = mx - mn;
    if (!(mx > 0.0) || !(c > 0.0)) {
        return vec3<f32>(0.0, 0.0, mx);
    }
    var h: f32;
    if (mx == p.x) {
        h = 60.0 * ((p.y - p.z) / c);
    } else if (mx == p.y) {
        h = 60.0 * ((p.z - p.x) / c + 2.0);
    } else {
        h = 60.0 * ((p.x - p.y) / c + 4.0);
    }
    return vec3<f32>(wrap(h), c / mx, mx);
}

fn from_hsv(h: f32, s: f32, v: f32) -> vec3<f32> {
    let c = v * s;
    let hp = wrap(h) / 60.0;
    let x = c * (1.0 - abs(hp % 2.0 - 1.0));
    let m = v - c;
    var o: vec3<f32>;
    switch (u32(hp)) {
        case 0u: { o = vec3<f32>(c, x, 0.0); }
        case 1u: { o = vec3<f32>(x, c, 0.0); }
        case 2u: { o = vec3<f32>(0.0, c, x); }
        case 3u: { o = vec3<f32>(0.0, x, c); }
        case 4u: { o = vec3<f32>(x, 0.0, c); }
        default: { o = vec3<f32>(c, 0.0, x); }
    }
    return o + vec3<f32>(m);
}

fn node(i: u32) -> vec2<f32> {
    let v = plan[4u + i / 2u];
    if (i % 2u == 0u) {
        return v.xy;
    }
    return v.zw;
}

fn cell_index(x: f32, n: u32) -> u32 {
    let y = clamp(x, 0.0, 1.0) * f32(n - 1u);
    return min(u32(floor(y)), n - 2u);
}

fn cell_fraction(x: f32, n: u32, i: u32) -> f32 {
    return clamp(x, 0.0, 1.0) * f32(n - 1u) - f32(i);
}

fn lookup(h: f32, s: f32, vc: f32) -> vec2<f32> {
    let hx = wrap(h) / (360.0 / f32(HUES));
    let h0 = u32(floor(hx)) % HUES;
    let fh = hx - floor(hx);
    let h1 = (h0 + 1u) % HUES;
    let s0 = cell_index(s, SATURATIONS);
    let fs = cell_fraction(s, SATURATIONS, s0);
    let v0 = cell_index(vc, VALUES);
    let fv = cell_fraction(vc, VALUES, v0);
    var out = vec2<f32>(0.0, 0.0);
    for (var a = 0u; a < 2u; a = a + 1u) {
        let v = v0 + a;
        let wv = select(1.0 - fv, fv, a == 1u);
        for (var b = 0u; b < 2u; b = b + 1u) {
            let hh = select(h0, h1, b == 1u);
            let wh = select(1.0 - fh, fh, b == 1u);
            for (var d = 0u; d < 2u; d = d + 1u) {
                let ss = s0 + d;
                let ws = select(1.0 - fs, fs, d == 1u);
                let w = wv * wh * ws;
                let n = node((v * HUES + hh) * SATURATIONS + ss);
                out.x = out.x + w * n.x;
                out.y = out.y + w * n.y;
            }
        }
    }
    return out;
}

fn table(p: vec3<f32>) -> vec3<f32> {
    let n = plan[3].xyz;
    let l = luma(p);
    if (!(l > 0.0)) {
        return p;
    }
    let q = hsv(vec3<f32>(p.x / n.x, p.y / n.y, p.z / n.z));
    if (!(q.y > 0.0)) {
        return p;
    }
    let vv = max(q.z, 0.0);
    let t = lookup(q.x, q.y, sqrt(vv / (1.0 + vv)));
    let r = from_hsv(q.x + t.x, q.y * t.y, q.z);
    let o = vec3<f32>(r.x * n.x, r.y * n.y, r.z * n.z);
    let lo = luma(o);
    if (!(lo > 0.0)) {
        return p;
    }
    let k = l / lo;
    return o * k;
}

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
    var o: vec3<f32>;
    for (var c = 0u; c < 3u; c = c + 1u) {
        o[c] = plan[c].x * r + plan[c].y * g + plan[c].z * b;
    }
    if (plan[3].w > 0.0) {
        o = table(o);
    }
    output[base] = o.x;
    output[base + 1u] = o.y;
    output[base + 2u] = o.z;
    output[base + 3u] = input[base + 3u];
}
