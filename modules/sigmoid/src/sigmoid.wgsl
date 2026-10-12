// Sigmoid's GPU variant: the same curve, negative-value desaturation and
// hue preservation as the CPU code (itself darktable's per-channel method).

@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
// [0]: white, paper exposure, film fog, film power; [1].x: paper power,
// [1].y: hue preservation.
@group(0) @binding(2) var<uniform> plan: array<vec4<f32>, 2>;
@group(0) @binding(3) var<uniform> shape: array<vec4<u32>, 2>;

fn curve(x: f32) -> f32 {
    let film = pow(plan[0].z + max(x, 0.0), plan[0].w);
    let v = plan[0].x * pow(film / (plan[0].y + film), plan[1].x);
    return select(v, plan[0].x, v != v);
}

fn desaturate_negative(p: vec3<f32>) -> vec3<f32> {
    let average = max((p.x + p.y + p.z) / 3.0, 0.0);
    let lowest = min(p.x, min(p.y, p.z));
    var factor = 1.0;
    if (lowest < 0.0) {
        factor = -average / (lowest - average);
    }
    return vec3<f32>(average) + factor * (p - vec3<f32>(average));
}

// Indices of the largest, middle and smallest channels, as darktable orders them.
fn order(p: vec3<f32>) -> vec3<u32> {
    if (p.x >= p.y) {
        if (p.y > p.z) { return vec3<u32>(0u, 1u, 2u); }
        if (p.z > p.x) { return vec3<u32>(2u, 0u, 1u); }
        if (p.z > p.y) { return vec3<u32>(0u, 2u, 1u); }
        return vec3<u32>(0u, 1u, 2u);
    }
    if (p.x >= p.z) { return vec3<u32>(1u, 0u, 2u); }
    if (p.z > p.y) { return vec3<u32>(2u, 1u, 0u); }
    return vec3<u32>(1u, 2u, 0u);
}

fn preserve_hue(input: vec3<f32>, curved: vec3<f32>, hue: f32) -> vec3<f32> {
    let o = order(input);
    let mx = o.x;
    let md = o.y;
    let mn = o.z;
    let chroma = input[mx] - input[mn];
    var midscale = 0.0;
    if (chroma != 0.0) {
        midscale = (input[md] - input[mn]) / chroma;
    }
    let full = curved[mn] + (curved[mx] - curved[mn]) * midscale;
    let naive_mid = (1.0 - hue) * curved[md] + hue * full;
    let energy = curved.x + curved.y + curved.z;
    let naive_energy = curved[mn] + naive_mid + curved[mx];
    let min_plus_mid = input[mn] + input[md];
    var blend = 0.0;
    if (min_plus_mid != 0.0) {
        blend = 2.0 * input[mn] / min_plus_mid;
    }
    let aim = blend * energy + (1.0 - blend) * naive_energy;
    var out = vec3<f32>(0.0);
    if (naive_mid <= curved[md]) {
        let corrected = ((1.0 - hue) * curved[md]
            + hue * (midscale * curved[mx] + (1.0 - midscale) * (aim - curved[mx])))
            / (1.0 + hue * (1.0 - midscale));
        out[mn] = aim - curved[mx] - corrected;
        out[md] = corrected;
        out[mx] = curved[mx];
    } else {
        let corrected = ((1.0 - hue) * curved[md]
            + hue * (curved[mn] * (1.0 - midscale) + midscale * (aim - curved[mn])))
            / (1.0 + hue * midscale);
        out[mn] = curved[mn];
        out[md] = corrected;
        out[mx] = aim - curved[mn] - corrected;
    }
    return out;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.y * shape[0].w + id.x;
    if (p >= shape[0].x) {
        return;
    }
    let b = p * 4u;
    let rgb = desaturate_negative(vec3<f32>(input[b], input[b + 1u], input[b + 2u]));
    let curved = vec3<f32>(curve(rgb.x), curve(rgb.y), curve(rgb.z));
    let out = preserve_hue(rgb, curved, plan[1].y);
    output[b] = out.x;
    output[b + 1u] = out.y;
    output[b + 2u] = out.z;
    output[b + 3u] = input[b + 3u];
}
