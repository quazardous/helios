// The drawn mask shapes' GPU variants (circle, ellipse, gradient): the
// same profiles and box filter as the CPU code (shape.rs), in f32.
//
// plan[0], plan[1]: the shape's geometry; plan[2]: half a render pixel in
// image pixels, the render scale, the opacity, the inversion (1 or 0).
// The helpers up to `member` are the same in path.wgsl.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 3>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes; [2]: output and input origins (i32).
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 3>;

// Where pixel `p` of the output lies, in image pixels: its centre at the
// render scale `s`.
fn centre(p: u32, s: f32) -> vec2<f32> {
    let w = shape[1].z;
    let x = bitcast<i32>(shape[2].x) + i32(p % w);
    let y = bitcast<i32>(shape[2].y) + i32(p / w);
    return vec2<f32>((f32(x) + 0.5) / s, (f32(y) + 0.5) / s);
}

// The error function, Abramowitz and Stegun 7.1.26, as the CPU code has it.
fn erf(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.3275911 * abs(x));
    let poly = ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t;
    let y = 1.0 - poly * exp(-x * x);
    return select(y, -y, x < 0.0);
}

// A profile between its knots: 0 radial (p0 the inner radius, p1 the
// outer), 1 a falloff to 0 over p1, 2 a linear ramp p1 wide, 3 a sigmoid.
fn profile(kind: u32, u: f32, p0: f32, p1: f32) -> f32 {
    switch kind {
        case 0u: {
            let q = (p1 - u) * (p1 + u) / ((p1 - p0) * (p1 + p0));
            return q * q;
        }
        case 1u: {
            return 1.0 - u / p1;
        }
        case 2u: {
            return 0.5 + u / p1;
        }
        default: {
            return 0.5 + 0.5 * erf(2.0 * u / p1);
        }
    }
}

const G: f32 = 0.7745966692414834;

// The mean of a profile over [u - h, u + h], `below` up to `lo` and
// `above` from `hi`: the CPU code's box filter, with the window's bounds
// relative to u (large image coordinates keep their precision).
fn mean(kind: u32, u: f32, h: f32, lo: f32, hi: f32, below: f32, above: f32, p0: f32, p1: f32, pieces: u32) -> f32 {
    let l = lo - u;
    let r = hi - u;
    if (h <= l) {
        return below;
    }
    if (-h >= r) {
        return above;
    }
    let m0 = max(-h, l);
    let m1 = min(h, r);
    var sum = below * (m0 + h) + above * (h - m1);
    if (m1 > m0) {
        let step = (m1 - m0) / f32(pieces);
        let half = step * 0.5;
        for (var k = 0u; k < pieces; k = k + 1u) {
            let c = u + m0 + (f32(k) + 0.5) * step;
            sum = sum + half * (5.0 / 9.0 * profile(kind, c - half * G, p0, p1)
                + 8.0 / 9.0 * profile(kind, c, p0, p1)
                + 5.0 / 9.0 * profile(kind, c + half * G, p0, p1));
        }
    }
    return sum / (2.0 * h);
}

// A member's inversion, then its opacity.
fn member(v: f32, opacity: f32, invert: f32) -> f32 {
    return select(v, 1.0 - v, invert > 0.5) * opacity;
}

// Radius of the ellipse of half-axes a, b along a direction whose cosine
// and sine in the ellipse's frame are c, s.
fn ellipse_radius(a: f32, b: f32, c: f32, s: f32) -> f32 {
    let d = a * a * s * s + b * b * c * c;
    return select(0.0, a * b / sqrt(d), d > 0.0);
}

// Geometry: centre x, y, radius, outer radius.
@compute @workgroup_size(64)
fn circle(@builtin(global_invocation_id) id: vec3<u32>) {
    // The placeholder input is bound, so it must be used (layouts come
    // from the shader).
    _ = input[0];
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let h = plan[2].x;
    let l = length(centre(p, plan[2].y) - plan[0].xy);
    let r = plan[0].z;
    let t = plan[0].w;
    output[p] = member(mean(0u, l, h, r, t, 1.0, 0.0, r, t, 1u), plan[2].z, plan[2].w);
}

// Geometry: centre x, y, inner half-axes, outer half-axes; cosine and sine
// of the rotation.
@compute @workgroup_size(64)
fn ellipse(@builtin(global_invocation_id) id: vec3<u32>) {
    // The placeholder input is bound, so it must be used (layouts come
    // from the shader).
    _ = input[0];
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let h = plan[2].x;
    let d = centre(p, plan[2].y) - plan[0].xy;
    let l = length(d);
    var n = vec2<f32>(0.0, 1.0);
    if (l > 0.0) {
        n = d / l;
    }
    let cs = plan[1].z;
    let sn = plan[1].w;
    let c = n.x * cs + n.y * sn;
    let s = -n.x * sn + n.y * cs;
    let r = ellipse_radius(plan[0].z, plan[0].w, c, s);
    let t = ellipse_radius(plan[1].x, plan[1].y, c, s);
    output[p] = member(mean(0u, l, h, r, t, 1.0, 0.0, r, t, 1u), plan[2].z, plan[2].w);
}

// Geometry: anchor x, y, unit normal to the full side, transition width,
// sigmoid (1) or linear (0).
@compute @workgroup_size(64)
fn gradient(@builtin(global_invocation_id) id: vec3<u32>) {
    // The placeholder input is bound, so it must be used (layouts come
    // from the shader).
    _ = input[0];
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let h = plan[2].x;
    let d = dot(centre(p, plan[2].y) - plan[0].xy, plan[0].zw);
    let w = plan[1].x;
    var v: f32;
    if (plan[1].y > 0.5) {
        v = mean(3u, d, h, -2.0 * w, 2.0 * w, 0.0, 1.0, 0.0, w, 4u);
    } else {
        v = mean(2u, d, h, -0.5 * w, 0.5 * w, 0.0, 1.0, 0.0, w, 1u);
    }
    output[p] = member(v, plan[2].z, plan[2].w);
}
