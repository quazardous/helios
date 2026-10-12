// The drawn mask path's GPU variant: the same polygon (flattened by the
// CPU's plan), even-odd rule and linear falloff as the CPU code
// (shape.rs), in f32, every side read for every pixel.
//
// plan[0]: points, feather; plan[1]: half a render pixel in image pixels,
// the render scale, the opacity, the inversion (1 or 0); then the points,
// two a vec4. The helpers are those of shapes.wgsl.

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 1024>;
// [0]: pixels, input channels, output channels, invocations per grid row;
// [1]: input and output sizes; [2]: output and input origins (i32).
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 3>;

fn centre(p: u32, s: f32) -> vec2<f32> {
    let w = shape[1].z;
    let x = bitcast<i32>(shape[2].x) + i32(p % w);
    let y = bitcast<i32>(shape[2].y) + i32(p / w);
    return vec2<f32>((f32(x) + 0.5) / s, (f32(y) + 0.5) / s);
}

const G: f32 = 0.7745966692414834;

// The mean of the falloff (1 up to 0, 0 from `feather`) over [d - h, d + h].
fn falloff(d: f32, h: f32, feather: f32) -> f32 {
    let l = -d;
    let r = feather - d;
    if (h <= l) {
        return 1.0;
    }
    if (-h >= r) {
        return 0.0;
    }
    let m0 = max(-h, l);
    let m1 = min(h, r);
    var sum = m0 + h;
    if (m1 > m0) {
        let half = (m1 - m0) * 0.5;
        let c = d + m0 + half;
        sum = sum + half * (5.0 / 9.0 * (1.0 - (c - half * G) / feather)
            + 8.0 / 9.0 * (1.0 - c / feather)
            + 5.0 / 9.0 * (1.0 - (c + half * G) / feather));
    }
    return sum / (2.0 * h);
}

fn member(v: f32, opacity: f32, invert: f32) -> f32 {
    return select(v, 1.0 - v, invert > 0.5) * opacity;
}

fn point(i: u32) -> vec2<f32> {
    let v = plan[2u + i / 2u];
    return select(v.zw, v.xy, i % 2u == 0u);
}

fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = b - a;
    let q = p - a;
    let len2 = dot(d, d);
    var t = 0.0;
    if (len2 > 0.0) {
        t = clamp(dot(q, d) / len2, 0.0, 1.0);
    }
    return length(q - t * d);
}

@compute @workgroup_size(64)
fn path(@builtin(global_invocation_id) id: vec3<u32>) {
    // The placeholder input is bound, so it must be used (layouts come
    // from the shader).
    _ = input[0];
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let n = u32(plan[0].x);
    let feather = plan[0].y;
    let h = plan[1].x;
    if (n < 3u) {
        output[p] = member(0.0, plan[1].z, plan[1].w);
        return;
    }
    let c = centre(p, plan[1].y);
    var inside = false;
    var dist = 3.0e38;
    for (var i = 0u; i < n; i = i + 1u) {
        let a = point(i);
        let b = point((i + 1u) % n);
        if ((a.y <= c.y) != (b.y <= c.y)) {
            if (c.x > a.x + (c.y - a.y) / (b.y - a.y) * (b.x - a.x)) {
                inside = !inside;
            }
        }
        dist = min(dist, segment_distance(c, a, b));
    }
    let d = select(dist, -dist, inside);
    output[p] = member(falloff(d, h, feather), plan[1].z, plan[1].w);
}
