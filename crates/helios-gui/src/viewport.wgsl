// The image viewport: one layer of a float image (linear Rec.2020, the
// working space) drawn in a rectangle, then the display transform, a
// separate last step: to the display's primaries (sRGB for now), clipped,
// encoded when the target does not encode itself.
//
// Two layers are drawn: the base (a proxy of the whole image, which also
// paints the background) and the overlay (the exact render of what is
// visible, discarded outside its region).

struct View {
    rect: vec4<f32>,    // the viewport: x, y, width, height, in target pixels
    look: vec4<f32>,    // view centre in image pixels (x, y), zoom (target pixels per image pixel), encode (1: apply the sRGB curve)
    layer: vec4<f32>,   // the layer's top left in image pixels (x, y), image pixels per texel, overlay (1) or base (0)
    sizes: vec4<f32>,   // the layer's texture size (w, h), the image's size (w, h)
    m0: vec4<f32>,      // working space to the display's primaries, by rows
    m1: vec4<f32>,
    m2: vec4<f32>,
    background: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var image: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the target; egui clips it to the viewport.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn srgb(v: vec3<f32>) -> vec3<f32> {
    let lo = v * 12.92;
    let hi = 1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, v <= vec3<f32>(0.0031308));
}

/// The display transform: primaries, clipping, encoding.
fn display(c: vec3<f32>) -> vec3<f32> {
    let d = vec3<f32>(dot(view.m0.xyz, c), dot(view.m1.xyz, c), dot(view.m2.xyz, c));
    let clipped = clamp(d, vec3<f32>(0.0), vec3<f32>(1.0));
    if (view.look.w > 0.5) {
        return srgb(clipped);
    }
    return clipped;
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let centre = view.rect.xy + view.rect.zw * 0.5;
    let q = view.look.xy + (at.xy - centre) / view.look.z;
    let overlay = view.layer.w > 0.5;
    if (q.x < 0.0 || q.y < 0.0 || q.x >= view.sizes.z || q.y >= view.sizes.w) {
        if (overlay) {
            discard;
        }
        return view.background;
    }
    let t = (q - view.layer.xy) / view.layer.z;
    if (t.x < 0.0 || t.y < 0.0 || t.x >= view.sizes.x || t.y >= view.sizes.y) {
        if (overlay) {
            discard;
        }
        return view.background;
    }
    let c = textureLoad(image, vec2<i32>(t), 0).rgb;
    return vec4<f32>(display(c), 1.0);
}
