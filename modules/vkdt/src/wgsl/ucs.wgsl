// Ported from vkdt (https://github.com/hanatos/vkdt, commit 0e0d098):
// src/pipe/modules/shared/dtucs.glsl, src/pipe/modules/colourspaces.glsl,
// src/pipe/modules/matrices.h. vkdt: copyright 2019 johannes hanika, under
// the BSD-2-Clause licence; its notice is in modules/vkdt/LICENSE-vkdt.
// darktable Uniform Color Space 2022: Aurélien Pierre, from darktable,
// under the MIT licence (as vkdt's dtucs.glsl states).
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-2-Clause AND MIT

// Shared colour functions, put in front of the kernels that use them:
// linear Rec.2020 to and from xyY, and darktable UCS 2022 (dt UCS) JCH.
// WGSL matrices are built from columns, as GLSL's.

fn rec2020_to_xyy(rgb: vec3<f32>) -> vec3<f32> {
    let m = mat3x3<f32>(
        vec3<f32>(0.636958048301290991, 0.26270021201126692, 4.9999999999999999e-17),
        vec3<f32>(0.144616903586208406, 0.677998071518871148, 0.0280726930490874452),
        vec3<f32>(0.168880975164172054, 0.0593017164698619384, 1.06098505771079066));
    let xyz = m * rgb;
    let sum = xyz.x + xyz.y + xyz.z;
    return vec3<f32>(xyz.x / sum, xyz.y / sum, xyz.y);
}

fn xyy_to_rec2020(xyy: vec3<f32>) -> vec3<f32> {
    let s = xyy.z / xyy.y;
    let xyz = vec3<f32>(xyy.x * s, xyy.z, (1.0 - xyy.x - xyy.y) * s);
    let m = mat3x3<f32>(
        vec3<f32>(1.71665119, -0.66668435, 0.01763986),
        vec3<f32>(-0.35567078, 1.61648124, -0.04277061),
        vec3<f32>(-0.25336628, 0.01576855, 0.94210312));
    return m * xyz;
}

fn dt_ucs_y_to_l_star(y: f32) -> f32 {
    let y_hat = pow(y, 0.631651345306265);
    return 2.098883786377 * y_hat / (y_hat + 1.12426773749357);
}

fn dt_ucs_l_star_to_y(l_star: f32) -> f32 {
    return pow(1.12426773749357 * l_star / (2.098883786377 - l_star), 1.5831518565279648);
}

const DT_UCS_FACTORS = vec2<f32>(1.39656225667, 1.4513954287);
const DT_UCS_HALF_VALUES = vec2<f32>(1.49217352929, 1.52488637914);

fn dt_ucs_xy_to_uv(xy: vec2<f32>) -> vec2<f32> {
    let m1 = mat3x3<f32>(
        vec3<f32>(-0.783941002840055, 0.745273540913283, 0.318707282433486),
        vec3<f32>(0.277512987809202, -0.205375866083878, 2.16743692732158),
        vec3<f32>(0.153836578598858, -0.165478376301988, 0.291320554395942));
    let uvd = m1 * vec3<f32>(xy, 1.0);
    let uv = uvd.xy / uvd.z;
    let star = DT_UCS_FACTORS * uv / (abs(uv) + DT_UCS_HALF_VALUES);
    let m2 = mat2x2<f32>(
        vec2<f32>(-1.124983854323892, 1.86323315098672),
        vec2<f32>(-0.980483721769325, 1.971853092390862));
    return m2 * star;
}

fn xyy_to_dt_ucs_jch(xyy: vec3<f32>, l_white: f32) -> vec3<f32> {
    let uv = dt_ucs_xy_to_uv(xyy.xy);
    let l_star = dt_ucs_y_to_l_star(xyy.z);
    let m2 = dot(uv, uv);
    return vec3<f32>(
        l_star / l_white,
        15.932993652962535 * pow(l_star, 0.6523997524738018) * pow(m2, 0.6007557017508491) / l_white,
        atan2(uv.y, uv.x));
}

fn dt_ucs_jch_to_xyy(jch: vec3<f32>, l_white: f32) -> vec3<f32> {
    let l_star = jch.x * l_white;
    var m = pow(jch.y * l_white / (15.932993652962535 * pow(l_star, 0.6523997524738018)), 0.8322850678616855);
    m = clamp(m, 0.0, 0.05);
    let m1 = mat2x2<f32>(
        vec2<f32>(-5.037522385190711, 4.760029407436461),
        vec2<f32>(-2.504856328185843, 2.874012963239247));
    let star = m1 * (m * vec2<f32>(cos(jch.z), sin(jch.z)));
    let uv = -DT_UCS_HALF_VALUES * star / (abs(star) - DT_UCS_FACTORS);
    let m2 = mat3x3<f32>(
        vec3<f32>(0.167171472114775, -0.150959086409163, 0.940254742367256),
        vec3<f32>(0.141299802443708, -0.155185060382272, 1.000000000000000),
        vec3<f32>(-0.00801531300850582, -0.00843312433578007, -0.0256325967652889));
    let xyd = m2 * vec3<f32>(uv, 1.0);
    return vec3<f32>(xyd.xy / xyd.z, dt_ucs_l_star_to_y(l_star));
}
