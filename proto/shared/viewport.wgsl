// A float image (linear working space) drawn in a rectangle: pan, zoom,
// clipping to [0, 1] and the sRGB curve when the target does not apply it.
// The same shader serves both prototypes.

struct View {
    origin: vec2<f32>, // top left of the viewport, in target pixels
    pan: vec2<f32>,    // image offset inside it, in target pixels
    zoom: f32,         // target pixels per image pixel
    encode: f32,       // 1 when the target expects sRGB-encoded values
    size: vec2<f32>,   // image size in pixels
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var image: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the viewport.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn srgb(v: vec3<f32>) -> vec3<f32> {
    let lo = v * 12.92;
    let hi = 1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, v <= vec3<f32>(0.0031308));
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let p = (at.xy - view.origin - view.pan) / view.zoom;
    if (p.x < 0.0 || p.y < 0.0 || p.x >= view.size.x || p.y >= view.size.y) {
        return vec4<f32>(0.11, 0.11, 0.12, 1.0);
    }
    let c = clamp(textureLoad(image, vec2<i32>(p), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    // The working space is Rec.2020; a color-managed path converts to the
    // display's space here (prototype: shown as is).
    var out = c;
    if (view.encode > 0.5) {
        out = srgb(c);
    }
    return vec4<f32>(out, 1.0);
}
