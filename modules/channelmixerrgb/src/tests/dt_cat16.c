/* darktable's CAT16 path of channelmixerrgb, copied from
   src/iop/channelmixerrgb.c, src/common/chromatic_adaptation.h,
   src/common/illuminants.h and colorspaces_inline_conversions.h
   (darktable fork 199c797d2), stripped of SIMD macros, to print golden
   values for Helios's parity test. */
#include <stdio.h>
#include <math.h>
#include <float.h>
#define TRUE 1
#define FALSE 0
typedef int gboolean;
typedef float dt_aligned_pixel_t[4];
typedef float dt_colormatrix_t[4][4];
#define NORM_MIN 1.52587890625e-05f
#define INVERSE_SQRT_3 0.5773502691896258f
#define DT_FMA(x, y, z) fmaf(x, y, z)
#define for_three_channels(c) for(size_t c = 0; c < 3; c++)
#define for_four_channels(c) for(size_t c = 0; c < 4; c++)
static const struct { float x, y; } D50xyY = {0.34567, 0.35850};
static inline float sqf(float x) { return x * x; }

static void dt_apply_transposed_color_matrix(const dt_aligned_pixel_t in, const dt_colormatrix_t M, dt_aligned_pixel_t out)
{ for_four_channels(r) out[r] = M[0][r] * in[0] + M[1][r] * in[1] + M[2][r] * in[2]; }
static void dt_colormatrix_mul(dt_colormatrix_t dst, const dt_colormatrix_t m1, const dt_colormatrix_t m2)
{ for(int k = 0; k < 3; ++k) { for(int i = 0; i < 3; ++i) { float x = 0.0f; for(int j = 0; j < 3; j++) x += m1[k][j] * m2[j][i]; dst[k][i] = x; } dst[k][3] = 0.0f; } for(int c = 0; c < 4; c++) dst[3][c] = 0.f; }
static void dt_colormatrix_transpose(dt_colormatrix_t dst, const dt_colormatrix_t src)
{ for(int i = 0; i < 4; i++) for(int j = 0; j < 4; j++) dst[i][j] = src[j][i]; }
static void dt_vector_max_nan(dt_aligned_pixel_t max, const dt_aligned_pixel_t v1, const dt_aligned_pixel_t v2)
{ for_four_channels(c) max[c] = fmaxf(v1[c], v2[c]); }
static void dt_vector_clipneg_nan(dt_aligned_pixel_t values)
{ static const dt_aligned_pixel_t zero = { 0.0f, 0.0f, 0.0f, 0.0f }; dt_vector_max_nan(values, values, zero); }
static float scalar_product(const dt_aligned_pixel_t v1, const dt_aligned_pixel_t v2)
{ float acc = 0.f; for_three_channels(c) acc += v1[c] * v2[c]; return acc; }
static float euclidean_norm(const dt_aligned_pixel_t vector)
{ return fmaxf(sqrtf(sqf(vector[0]) + sqf(vector[1]) + sqf(vector[2])), NORM_MIN); }

static const dt_colormatrix_t XYZ_to_CAT16_LMS = { {  0.401288f, 0.650173f, -0.051461f, 0.f },
                                                   { -0.250268f, 1.204414f,  0.045854f, 0.f },
                                                   { -0.002079f, 0.048952f,  0.953127f, 0.f } };
static const dt_colormatrix_t XYZ_to_CAT16_LMS_trans =
  { {  0.401288f, -0.250268f, -0.002079f, 0.f },
    {  0.650173f,  1.204414f,  0.048952f, 0.f },
    { -0.051461f,  0.045854f,  0.953127f, 0.f } };
static const dt_colormatrix_t CAT16_LMS_to_XYZ = { {  1.862068f, -1.011255f,  0.149187f, 0.f },
                                                   {  0.38752f ,  0.621447f, -0.008974f, 0.f },
                                                   { -0.015841f, -0.034123f,  1.049964f, 0.f } };
static const dt_colormatrix_t CAT16_LMS_to_XYZ_trans =
  { {  1.862068f,  0.38752f , -0.015841f, 0.f },
    { -1.011255f,  0.621447f, -0.034123f, 0.f },
    {  0.149187f, -0.008974f,  1.049964f, 0.f } };
static void convert_XYZ_to_CAT16_LMS(const dt_aligned_pixel_t XYZ, dt_aligned_pixel_t LMS)
{ dt_apply_transposed_color_matrix(XYZ, XYZ_to_CAT16_LMS_trans, LMS); }
static void make_RGB_to_CAT16_LMS(const dt_colormatrix_t rgb, dt_colormatrix_t lms)
{ dt_colormatrix_mul(lms, XYZ_to_CAT16_LMS, rgb); }
static void convert_CAT16_LMS_to_XYZ(const dt_aligned_pixel_t LMS, dt_aligned_pixel_t XYZ)
{ dt_apply_transposed_color_matrix(LMS, CAT16_LMS_to_XYZ_trans, XYZ); }
static void make_CAT16_LMS_to_XYZ(const dt_colormatrix_t lms, dt_colormatrix_t xyz)
{ dt_colormatrix_mul(xyz, CAT16_LMS_to_XYZ, lms); }
static void CAT16_adapt_D50(const dt_aligned_pixel_t lms_in, const dt_aligned_pixel_t origin_illuminant,
                            const float D, const int full, dt_aligned_pixel_t lms_out)
{
  const dt_aligned_pixel_t D50 = { 0.994535f, 1.000997f, 0.833036f, 0.f };
  if(full)
  {
    lms_out[0] = lms_in[0] * D50[0] / origin_illuminant[0];
    lms_out[1] = lms_in[1] * D50[1] / origin_illuminant[1];
    lms_out[2] = lms_in[2] * D50[2] / origin_illuminant[2];
  }
  else
  {
    lms_out[0] = lms_in[0] * (D * D50[0] / origin_illuminant[0] + 1.f - D);
    lms_out[1] = lms_in[1] * (D * D50[1] / origin_illuminant[1] + 1.f - D);
    lms_out[2] = lms_in[2] * (D * D50[2] / origin_illuminant[2] + 1.f - D);
  }
}
static void dt_xyY_to_XYZ(const dt_aligned_pixel_t xyY, dt_aligned_pixel_t XYZ)
{
  const gboolean bad = xyY[1] == 0.0f;
  XYZ[0] = bad ? 0.0f : xyY[2] * xyY[0] / xyY[1];
  XYZ[1] = bad ? 0.0f : xyY[2];
  XYZ[2] = bad ? 0.0f : xyY[2] * (1.f - xyY[0] - xyY[1]) / xyY[1];
}
static void dt_xyY_to_uvY(const dt_aligned_pixel_t xyY, dt_aligned_pixel_t uvY)
{
  const float denominator = -2.f * xyY[0] + 12.f * xyY[1] + 3.f;
  uvY[0] = 4.f * xyY[0] / denominator;
  uvY[1] = 9.f * xyY[1] / denominator;
  uvY[2] = xyY[2];
}
static void dt_uvY_to_xyY(const dt_aligned_pixel_t uvY, dt_aligned_pixel_t xyY)
{
  const float denominator = 6.0f * uvY[0] - 16.f * uvY[1] + 12.0f;
  xyY[0] = 9.f * uvY[0] / denominator;
  xyY[1] = 4.f * uvY[1] / denominator;
  xyY[2] = uvY[2];
}
static void _gamut_mapping(const dt_aligned_pixel_t input, const float compression, const gboolean clip, dt_aligned_pixel_t output)
{
  const float sum = input[0] + input[1] + input[2];
  const float Y = input[1];
  dt_aligned_pixel_t xyY = {  sum > 0.0f ? input[0] / sum : D50xyY.x,
                              sum > 0.0f ? input[1] / sum : D50xyY.y,
                              Y, 0.0f };
  dt_aligned_pixel_t uvY;
  dt_xyY_to_uvY(xyY, uvY);
  const float D50[2] = { 0.20915914598542354f, 0.488075320769787f };
  const float delta[2] = { D50[0] - uvY[0], D50[1] - uvY[1] };
  const float Delta = Y * (sqf(delta[0]) + sqf(delta[1]));
  const float correction = (compression == 0.0f) ? 0.f : powf(Delta, compression);
  for(size_t c = 0; c < 2; c++)
  {
    const float tmp = DT_FMA(correction, delta[c], uvY[c]);
    uvY[c] = (uvY[c] > D50[c])  ? fmaxf(tmp, D50[c]) : fminf(tmp, D50[c]);
  }
  dt_uvY_to_xyY(uvY, xyY);
  if(clip) for(size_t c = 0; c < 2; c++) xyY[c] = fmaxf(xyY[c], 0.0f);
  xyY[1] = fmaxf(xyY[1], NORM_MIN);
  const float scale = xyY[0] + xyY[1];
  const int sanitize = (scale >= 1.f);
  for(size_t c = 0; c < 2; c++) xyY[c] = (sanitize) ? xyY[c] / scale : xyY[c];
  dt_xyY_to_XYZ(xyY, output);
}
static void _luma_chroma(const dt_aligned_pixel_t input, const dt_aligned_pixel_t saturation,
                         const dt_aligned_pixel_t lightness, dt_aligned_pixel_t output)
{ /* version 3 */
  float norm = euclidean_norm(input);
  const float avg = fmaxf((input[0] + input[1] + input[2]) / 3.0f, NORM_MIN);
  if(norm > 0.f && avg > 0.f)
  {
    const float mix = scalar_product(input, lightness);
    norm *= INVERSE_SQRT_3;
    for_three_channels(c) output[c] = input[c] / norm;
    float coeff_ratio = scalar_product(output, saturation) / 3.f;
    for_three_channels(c)
    {
      const float min_ratio = (output[c] < 0.0f) ? output[c] : 0.0f;
      const float output_inverse = 1.0f - output[c];
      output[c] = fmaxf(DT_FMA(output_inverse, coeff_ratio, output[c]), min_ratio);
    }
    norm /= euclidean_norm(output) * INVERSE_SQRT_3;
    norm *= fmaxf(1.f + mix / avg, 0.f);
    for_three_channels(c) output[c] *= norm;
  }
  else
    for_three_channels(c) output[c] = input[c];
}
static void CCT_to_xy_daylight(const float t, float *x, float *y)
{
  float x_temp = 0.f, y_temp = 0.f;
  if(t >= 4000.f && t <= 7000.0f)
    x_temp = ((-4.6070e9f / t + 2.9678e6f) / t + 0.09911e3f) / t + 0.244063f;
  else if(t > 7000.f && t <= 25000.f)
    x_temp = ((-2.0064e9f / t + 1.9018e6f) / t + 0.24748e3f) / t + 0.237040f;
  y_temp = (-3.f * x_temp + 2.87f) * x_temp - 0.275f;
  *x = x_temp; *y = y_temp;
}
static void CCT_to_xy_blackbody(const float t, float *x, float *y)
{
  float x_temp = 0.f, y_temp = 0.f;
  if(t >= 1667.f && t <= 4000.f)
    x_temp = ((-0.2661239e9f / t - 0.2343589e6f) / t  + 0.8776956e3f) / t + 0.179910f;
  else if(t > 4000.f && t <= 25000.f)
    x_temp = ((-3.0258469e9f / t + 2.1070379e6f) / t  + 0.2226347e3f) / t + 0.240390f;
  if(t >= 1667.f && t <= 2222.f)
    y_temp = ((-1.1063814f * x_temp - 1.34811020f) * x_temp + 2.18555832f) * x_temp - 0.20219683f;
  else if(t > 2222.f && t <= 4000.f)
    y_temp = ((-0.9549476f * x_temp - 1.37418593f) * x_temp + 2.09137015f) * x_temp - 0.16748867f;
  else if(t > 4000.f && t <= 25000.f)
    y_temp = (( 3.0817580f * x_temp - 5.87338670f) * x_temp + 3.75112997f) * x_temp - 0.37001483f;
  *x = x_temp; *y = y_temp;
}
static float fluorescent[12][2] = { { 0.31310f, 0.33727f }, { 0.37208f, 0.37529f }, { 0.40910f, 0.39430f },
  { 0.44018f, 0.40329f }, { 0.31379f, 0.34531f }, { 0.37790f, 0.38835f }, { 0.31292f, 0.32933f },
  { 0.34588f, 0.35875f }, { 0.37417f, 0.37281f }, { 0.34609f, 0.35986f }, { 0.38052f, 0.37713f },
  { 0.43695f, 0.40441f } };
enum { PIPE = 0, A = 1, D = 2, E = 3, F = 4, BB = 6, CUSTOM = 7 };
/* illuminant_to_xy, the cases Helios has; custom leaves x, y as they are */
static void illuminant_to_xy(int illuminant, float *x_out, float *y_out, float t, int fluo)
{
  float x = 0.f, y = 0.f;
  switch(illuminant)
  {
    case PIPE: x = D50xyY.x; y = D50xyY.y; break;
    case E: x = y = 1.f / 3.f; break;
    case A: x = 0.44757f; y = 0.40745f; break;
    case F: x = fluorescent[fluo][0]; y = fluorescent[fluo][1]; break;
    case D: CCT_to_xy_daylight(t, &x, &y); if(y != 0.f && x != 0.f) break;
    case BB: CCT_to_xy_blackbody(t, &x, &y); break;
    default: return;
  }
  if(x != 0.f && y != 0.f) { *x_out = x; *y_out = y; }
}

/* commit_params and _loop_switch, CAT16, identity mixer, no saturation,
   lightness or grey */
static void run(const float *in, float *out, size_t n, const dt_colormatrix_t RGB_to_XYZ,
                const dt_colormatrix_t XYZ_to_RGB, float x, float y, float gamut_p, gboolean clip)
{
  const dt_colormatrix_t MIX = { { 1, 0, 0, 0 }, { 0, 1, 0, 0 }, { 0, 0, 1, 0 } };
  const dt_aligned_pixel_t saturation = { 0 }, lightness = { 0 };
  const float gamut = (gamut_p == 0.f) ? gamut_p : 1.f / gamut_p;
  dt_aligned_pixel_t XYZ = { x / y, 1.f, (1.f - x - y) / y, 0 }, illuminant;
  convert_XYZ_to_CAT16_LMS(XYZ, illuminant);
  illuminant[3] = 0.f;
  dt_colormatrix_t RGB_to_LMS, MIX_to_XYZ;
  make_RGB_to_CAT16_LMS(RGB_to_XYZ, RGB_to_LMS);
  make_CAT16_LMS_to_XYZ(MIX, MIX_to_XYZ);
  const float minval = clip ? 0.0f : -FLT_MAX;
  const dt_aligned_pixel_t min_value = { minval, minval, minval, minval };
  dt_colormatrix_t RGB_to_LMS_trans, MIX_to_XYZ_trans, XYZ_to_RGB_trans;
  dt_colormatrix_transpose(RGB_to_LMS_trans, RGB_to_LMS);
  dt_colormatrix_transpose(MIX_to_XYZ_trans, MIX_to_XYZ);
  dt_colormatrix_transpose(XYZ_to_RGB_trans, XYZ_to_RGB);
  for(size_t k = 0; k < n * 4; k += 4)
  {
    dt_aligned_pixel_t temp_one, temp_two;
    dt_vector_max_nan(temp_two, &in[k], min_value);
    dt_apply_transposed_color_matrix(temp_two, RGB_to_LMS_trans, temp_one);
    CAT16_adapt_D50(temp_one, illuminant, 1.0f, TRUE, temp_two);
    dt_apply_transposed_color_matrix(temp_two, MIX_to_XYZ_trans, temp_one);
    if(clip) dt_vector_clipneg_nan(temp_one);
    _gamut_mapping(temp_one, gamut, clip, temp_two);
    convert_XYZ_to_CAT16_LMS(temp_two, temp_one);
    if(clip) dt_vector_clipneg_nan(temp_one);
    _luma_chroma(temp_one, saturation, lightness, temp_two);
    if(clip) dt_vector_clipneg_nan(temp_two);
    convert_CAT16_LMS_to_XYZ(temp_two, temp_one);
    if(clip) dt_vector_clipneg_nan(temp_one);
    dt_apply_transposed_color_matrix(temp_one, XYZ_to_RGB_trans, temp_two);
    if(clip) dt_vector_clipneg_nan(temp_two);
    temp_two[3] = in[k + 3];
    for_four_channels(c) out[k + c] = temp_two[c];
  }
}

/* Rec.2020 to XYZ D50 (Bradford to the ICC white), as lcms builds
   darktable's linear Rec.2020 profile; in double, as helios-colour. */
static void inv3(const double m[3][3], double o[3][3])
{
  double det = m[0][0]*(m[1][1]*m[2][2]-m[1][2]*m[2][1]) - m[0][1]*(m[1][0]*m[2][2]-m[1][2]*m[2][0]) + m[0][2]*(m[1][0]*m[2][1]-m[1][1]*m[2][0]);
  for(int i = 0; i < 3; i++) for(int j = 0; j < 3; j++)
  {
    int a = (j+1)%3, b = (j+2)%3, p = (i+1)%3, q = (i+2)%3;
    o[i][j] = (m[a][p]*m[b][q] - m[a][q]*m[b][p]) / det;
  }
}
static void mul3(const double a[3][3], const double b[3][3], double o[3][3])
{ for(int i=0;i<3;i++) for(int j=0;j<3;j++){ o[i][j]=0; for(int k=0;k<3;k++) o[i][j]+=a[i][k]*b[k][j]; } }
static void ap3(const double m[3][3], const double v[3], double o[3])
{ for(int i=0;i<3;i++) o[i]=m[i][0]*v[0]+m[i][1]*v[1]+m[i][2]*v[2]; }

int main(void)
{
  const double prim[3][2] = { { 0.708, 0.292 }, { 0.170, 0.797 }, { 0.131, 0.046 } };
  const double wxy[2] = { 0.3127, 0.3290 };
  double cols[3][3], icols[3][3], s[3], w[3] = { wxy[0]/wxy[1], 1, (1-wxy[0]-wxy[1])/wxy[1] }, toxyz[3][3];
  for(int p = 0; p < 3; p++) { cols[0][p] = prim[p][0]/prim[p][1]; cols[1][p] = 1; cols[2][p] = (1-prim[p][0]-prim[p][1])/prim[p][1]; }
  inv3(cols, icols); ap3(icols, w, s);
  for(int i=0;i<3;i++) for(int j=0;j<3;j++) toxyz[i][j] = cols[i][j]*s[j];
  const double BR[3][3] = { { 0.8951, 0.2664, -0.1614 }, { -0.7502, 1.7135, 0.0367 }, { 0.0389, -0.0685, 1.0296 } };
  const double d50[3] = { 0.9642, 1.0, 0.8249 };
  double src[3], dst[3], sc[3][3] = {{0}}, ibr[3][3], t1[3][3], ad[3][3], m[3][3];
  ap3(BR, w, src); ap3(BR, d50, dst);
  for(int i=0;i<3;i++) sc[i][i] = dst[i]/src[i];
  inv3(BR, ibr); mul3(sc, BR, t1); mul3(ibr, t1, ad); mul3(ad, toxyz, m);
  double mi[3][3]; inv3(m, mi);
  dt_colormatrix_t RGB_to_XYZ = {{0}}, XYZ_to_RGB = {{0}};
  for(int i=0;i<3;i++) for(int j=0;j<3;j++) { RGB_to_XYZ[i][j] = (float)m[i][j]; XYZ_to_RGB[i][j] = (float)mi[i][j]; }

  const float colours[][4] = {
    { 0.18f, 0.18f, 0.18f, 1 }, { 1.0f, 1.0f, 1.0f, 1 }, { 0.0f, 0.0f, 0.0f, 1 },
    { 0.8f, 0.1f, 0.05f, 1 }, { 0.05f, 0.6f, 0.1f, 1 }, { 0.02f, 0.05f, 0.9f, 1 },
    { 0.45f, 0.3f, 0.2f, 1 }, { 2.5f, 1.2f, 0.4f, 1 }, { -0.05f, 0.3f, 0.6f, 0.5f },
    { 0.001f, 0.002f, 0.0005f, 1 },
  };
  const size_t n = sizeof(colours) / sizeof(colours[0]);
  /* name, illuminant, temperature, fluo, custom x, y */
  struct { const char *name; int ill; float t; int fluo; float x, y; } ills[] = {
    { "a", A, 5003, 0, 0, 0 }, { "d65", D, 6504, 0, 0, 0 }, { "f2", F, 5003, 1, 0, 0 },
    { "custom", CUSTOM, 5003, 0, 0.40f, 0.38f }, { "bb3200", BB, 3200, 0, 0, 0 },
    { "d3500", D, 3500, 0, 0, 0 }, { "pipe", PIPE, 5003, 0, 0, 0 },
  };
  struct { float gamut; gboolean clip; } opts[] = { { 1.f, TRUE }, { 0.f, FALSE }, { 4.f, TRUE }, { 0.f, TRUE } };
  printf("// Generated by a C harness holding darktable's code (see the module's tests).\n");
  printf("pub const COLOURS: [[f32; 4]; %zu] = [\n", n);
  for(size_t i = 0; i < n; i++) printf("    [%.9e, %.9e, %.9e, %.9e],\n", colours[i][0], colours[i][1], colours[i][2], colours[i][3]);
  printf("];\n\n");
  printf("pub struct Case {\n    pub illuminant: &'static str,\n    pub temperature: f64,\n    pub fluo: &'static str,\n    pub x: f64,\n    pub y: f64,\n    pub gamut: f64,\n    pub clip: bool,\n    pub xy: [f32; 2],\n    pub out: [[f32; 4]; %zu],\n}\n\n", n);
  const char *fl[] = { "f1", "f2" };
  printf("pub const CASES: &[Case] = &[\n");
  for(size_t i = 0; i < sizeof(ills)/sizeof(ills[0]); i++)
    for(size_t o = 0; o < sizeof(opts)/sizeof(opts[0]); o++)
    {
      float x = ills[i].x, y = ills[i].y;
      illuminant_to_xy(ills[i].ill, &x, &y, ills[i].t, ills[i].fluo);
      float out[64][4];
      run(&colours[0][0], &out[0][0], n, RGB_to_XYZ, XYZ_to_RGB, x, y, opts[o].gamut, opts[o].clip);
      const char *name = ills[i].ill == A ? "a" : ills[i].ill == D ? "d" : ills[i].ill == F ? "f" : ills[i].ill == BB ? "bb" : ills[i].ill == PIPE ? "pipe" : "custom";
      printf("    Case {\n        illuminant: \"%s\",\n        temperature: %.9e,\n        fluo: \"%s\",\n        x: %.9e,\n        y: %.9e,\n        gamut: %.9e,\n        clip: %s,\n        xy: [%.9e, %.9e],\n        out: [\n",
             name, ills[i].t, fl[ills[i].fluo], ills[i].x, ills[i].y, opts[o].gamut, opts[o].clip ? "true" : "false", x, y);
      for(size_t k = 0; k < n; k++) printf("            [%.9e, %.9e, %.9e, %.9e],\n", out[k][0], out[k][1], out[k][2], out[k][3]);
      printf("        ],\n    },\n");
    }
  printf("];\n");
  return 0;
}
