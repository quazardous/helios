/* darktable's "as shot in camera" illuminant, copied from
   src/common/illuminants.h (find_illuminant_xy_from_as_shot_coeffs,
   find_illuminant_xy_from_wb_coeffs, WB_coeffs_to_illuminant_xy,
   matrice_pseudoinverse, _is_usable_illuminant_xy, xy_to_CCT,
   CCT_reverse_lookup, xy_to_uv), src/common/chromatic_adaptation.h (the
   Bradford and CAT16 conversions), src/iop/channelmixerrgb.c
   (_check_if_close_to_daylight) and src/common/colorspaces.c (the D65
   multipliers of dt_colorspaces_conversion_matrices_rgb), darktable fork
   199c797d2, stripped of SIMD and OpenMP macros, to print golden values
   for Helios's parity test. The camera matrices are LibRaw's
   (src/tables/colordata.cpp), as darktable has them. */
#include <stdio.h>
#include <math.h>
#include <float.h>
#define TRUE 1
#define FALSE 0
typedef int gboolean;
typedef float dt_aligned_pixel_t[4];
typedef float dt_colormatrix_t[4][4];

static void dt_apply_transposed_color_matrix(const dt_aligned_pixel_t in, const dt_colormatrix_t M, dt_aligned_pixel_t out)
{ for(int r = 0; r < 4; r++) out[r] = M[0][r] * in[0] + M[1][r] * in[1] + M[2][r] * in[2]; }

static const dt_colormatrix_t XYZ_to_Bradford_LMS_trans =
  { {  0.8951f, -0.7502f,  0.0389f, 0.f },
    {  0.2664f,  1.7135f, -0.0685f, 0.f },
    { -0.1614f,  0.0367f,  1.0296f, 0.f } };
static const dt_colormatrix_t Bradford_LMS_to_XYZ_trans =
  { {  0.9870f,  0.4323f, -0.0085f, 0.f },
    { -0.1471f,  0.5184f,  0.0400f, 0.f },
    {  0.1600f,  0.0493f,  0.9685f, 0.f } };
static const dt_colormatrix_t XYZ_to_CAT16_LMS_trans =
  { {  0.401288f, -0.250268f, -0.002079f, 0.f },
    {  0.650173f,  1.204414f,  0.048952f, 0.f },
    { -0.051461f,  0.045854f,  0.953127f, 0.f } };
static void convert_XYZ_to_bradford_LMS(const dt_aligned_pixel_t XYZ, dt_aligned_pixel_t LMS)
{ dt_apply_transposed_color_matrix(XYZ, XYZ_to_Bradford_LMS_trans, LMS); }
static void convert_bradford_LMS_to_XYZ(const dt_aligned_pixel_t LMS, dt_aligned_pixel_t XYZ)
{ dt_apply_transposed_color_matrix(LMS, Bradford_LMS_to_XYZ_trans, XYZ); }
static void convert_XYZ_to_CAT16_LMS(const dt_aligned_pixel_t XYZ, dt_aligned_pixel_t LMS)
{ dt_apply_transposed_color_matrix(XYZ, XYZ_to_CAT16_LMS_trans, LMS); }
static void bradford_adapt_D50(const dt_aligned_pixel_t lms_in, const dt_aligned_pixel_t origin_illuminant,
                               const float p, const int full, dt_aligned_pixel_t lms_out)
{
  const dt_aligned_pixel_t D50 = { 0.996078f, 1.020646f, 0.818155f, 0.f };
  dt_aligned_pixel_t temp = { lms_in[0] / origin_illuminant[0], lms_in[1] / origin_illuminant[1],
                              lms_in[2] / origin_illuminant[2], 0.f };
  if(full) temp[2] = (temp[2] > 0.f) ? powf(temp[2], p) : temp[2];
  lms_out[0] = D50[0] * temp[0];
  lms_out[1] = D50[1] * temp[1];
  lms_out[2] = D50[2] * temp[2];
}
static void illuminant_xy_to_XYZ(const float x, const float y, dt_aligned_pixel_t XYZ)
{ XYZ[0] = x / y; XYZ[1] = 1.f; XYZ[2] = (1.f - x - y) / y; }

static void WB_coeffs_to_illuminant_xy(const float CAM_to_XYZ[4][3], const dt_aligned_pixel_t WB, float *x, float *y)
{
  dt_aligned_pixel_t XYZ, LMS;
  XYZ[0] = CAM_to_XYZ[0][0] / WB[0] + CAM_to_XYZ[1][0] / WB[1] + CAM_to_XYZ[2][0] / WB[2];
  XYZ[1] = CAM_to_XYZ[0][1] / WB[0] + CAM_to_XYZ[1][1] / WB[1] + CAM_to_XYZ[2][1] / WB[2];
  XYZ[2] = CAM_to_XYZ[0][2] / WB[0] + CAM_to_XYZ[1][2] / WB[1] + CAM_to_XYZ[2][2] / WB[2];
  static const dt_aligned_pixel_t D65 = { 0.941238f, 1.040633f, 1.088932f, 0.f };
  const float p = powf(1.088932f / 0.818155f, 0.0834f);
  convert_XYZ_to_bradford_LMS(XYZ, LMS);
  bradford_adapt_D50(LMS, D65, p, FALSE, LMS);
  convert_bradford_LMS_to_XYZ(LMS, XYZ);
  XYZ[0] /= XYZ[1];
  XYZ[2] /= XYZ[1];
  XYZ[1] /= XYZ[1];
  *x = XYZ[0] / (XYZ[0] + XYZ[1] + XYZ[2]);
  *y = XYZ[1] / (XYZ[0] + XYZ[1] + XYZ[2]);
}

static void matrice_pseudoinverse(float (*in)[3], float (*out)[3], int size)
{
  float work[3][6];
  for(int i = 0; i < 3; i++)
  {
    for(int j = 0; j < 6; j++) work[i][j] = j == i+3;
    for(int j = 0; j < 3; j++) for(int k = 0; k < size; k++) work[i][j] += in[k][i] * in[k][j];
  }
  for(int i = 0; i < 3; i++)
  {
    float num = work[i][i];
    for(int j = 0; j < 6; j++) work[i][j] /= num;
    for(int k = 0; k < 3; k++)
    {
      if(k==i) continue;
      num = work[k][i];
      for(int j = 0; j < 6; j++) work[k][j] -= work[i][j] * num;
    }
  }
  for(int i = 0; i < size; i++)
    for(int j = 0; j < 3; j++)
    {
      out[i][j] = 0.0f;
      for(int k = 0; k < 3; k++) out[i][j] += work[j][k+3] * in[i][k];
    }
}

static gboolean _wb_coeffs_invalid(const float *wb_coeffs, const int num_coeffs)
{ for(int k = 0; k < num_coeffs; k++) if(!isnormal(wb_coeffs[k])) return TRUE; return FALSE; }

static gboolean _is_usable_illuminant_xy(const float x, const float y)
{
  const float z = 1.f - x - y;
  if(!(x > 0.f && y > 0.f && z > 0.f)) return FALSE;
  dt_aligned_pixel_t XYZ = { 0.f }, LMS = { 0.f };
  illuminant_xy_to_XYZ(x, y, XYZ);
  convert_XYZ_to_CAT16_LMS(XYZ, LMS);
  return LMS[0] > 0.f && LMS[1] > 0.f && LMS[2] > 0.f;
}

/* find_illuminant_xy_from_as_shot_coeffs with correction ratios of 1 (the
   white balance in camera reference mode) and a 3-colour sensor, the
   camera's matrix given: get_CAM_to_XYZ then find_illuminant_xy_from_wb_coeffs */
static gboolean as_shot_xy(float XYZ_to_CAM[4][3], const float *wb, float *chroma_x, float *chroma_y)
{
  if(_wb_coeffs_invalid(wb, 3)) return FALSE;
  float CAM_to_XYZ[4][3];
  matrice_pseudoinverse(XYZ_to_CAM, CAM_to_XYZ, 3);
  if(!isfinite(CAM_to_XYZ[0][0])) return FALSE;
  dt_aligned_pixel_t WB = { wb[0], wb[1], wb[2], 1.f };
  float x, y;
  WB_coeffs_to_illuminant_xy(CAM_to_XYZ, WB, &x, &y);
  if(!_is_usable_illuminant_xy(x, y)) return FALSE;
  *chroma_x = x;
  *chroma_y = y;
  return TRUE;
}

static float xy_to_CCT(const float x, const float y)
{
  if(x < FLT_MAX)
  {
    const float n = (x - 0.3366f)/(y - 0.1735f);
    return (-949.86315f + 6253.80338f * expf(-n / 0.92159f) + 28.70599f * expf(-n / 0.20039f)
            + 0.00004f * expf(-n / 0.07125f));
  }
  return 0.0f;
}
static void CCT_to_xy_daylight(const float t, float *x, float *y)
{
  float x_temp = 0.f, y_temp = 0.f;
  if(t >= 4000.f && t <= 7000.0f) x_temp = ((-4.6070e9f / t + 2.9678e6f) / t + 0.09911e3f) / t + 0.244063f;
  else if(t > 7000.f && t <= 25000.f) x_temp = ((-2.0064e9f / t + 1.9018e6f) / t + 0.24748e3f) / t + 0.237040f;
  y_temp = (-3.f * x_temp + 2.87f) * x_temp - 0.275f;
  *x = x_temp; *y = y_temp;
}
static void CCT_to_xy_blackbody(const float t, float *x, float *y)
{
  float x_temp = 0.f, y_temp = 0.f;
  if(t >= 1667.f && t <= 4000.f) x_temp = ((-0.2661239e9f / t - 0.2343589e6f) / t  + 0.8776956e3f) / t + 0.179910f;
  else if(t > 4000.f && t <= 25000.f) x_temp = ((-3.0258469e9f / t + 2.1070379e6f) / t  + 0.2226347e3f) / t + 0.240390f;
  if(t >= 1667.f && t <= 2222.f) y_temp = ((-1.1063814f * x_temp - 1.34811020f) * x_temp + 2.18555832f) * x_temp - 0.20219683f;
  else if(t > 2222.f && t <= 4000.f) y_temp = ((-0.9549476f * x_temp - 1.37418593f) * x_temp + 2.09137015f) * x_temp - 0.16748867f;
  else if(t > 4000.f && t <= 25000.f) y_temp = (( 3.0817580f * x_temp - 5.87338670f) * x_temp + 3.75112997f) * x_temp - 0.37001483f;
  *x = x_temp; *y = y_temp;
}
static void xy_to_uv(const float xy[2], float uv[2])
{
  const float denom = 12.f * xy[1] - 1.882f * xy[0] + 2.9088f;
  uv[0] = 5.5932f * xy[0] + 1.9116 * xy[1];
  uv[1] = 7.8972f * xy[1];
  uv[0] /= denom;
  uv[1] /= denom;
}
static float CCT_reverse_lookup(const float x, const float y)
{
  static const float T_min = 1667.f, T_max = 25000.f, T_range = T_max - T_min;
  static const size_t LUT_samples = 1<<16;
  float best = FLT_MAX, temperature = 0.f;
  for(size_t i = 0; i < LUT_samples; i++)
  {
    const float step = powf((float)i / (float)(LUT_samples - 1), 4.0f);
    const float T = T_min +  step * T_range;
    float x_bb, y_bb;
    if(T >= 4000.f) CCT_to_xy_daylight(T, &x_bb, &y_bb);
    else CCT_to_xy_blackbody(T, &x_bb, &y_bb);
    const float r = sqrtf((x_bb - x) * (x_bb - x) + (y_bb - y) * (y_bb - y));
    if(r < best) { best = r; temperature = T; }
  }
  return temperature;
}
/* illuminant_to_xy for D (falling back to BB) and BB */
static void daylight_or_bb(int bb, float t, float *x, float *y)
{
  if(!bb) { CCT_to_xy_daylight(t, x, y); if(*x != 0.f && *y != 0.f) return; }
  CCT_to_xy_blackbody(t, x, y);
}
/* _check_if_close_to_daylight: the temperature, and "d", "bb" or "custom" */
static const char *close_to_daylight(const float x, const float y, float *temperature)
{
  float t = xy_to_CCT(x, y);
  if(t < 3000.f && t > 1667.f) t = CCT_reverse_lookup(x, y);
  *temperature = t;
  const float xy_ref[2] = { x, y };
  float uv_ref[2]; xy_to_uv(xy_ref, uv_ref);
  float xy_test[2] = { 0.f }, uv_test[2];
  daylight_or_bb(0, t, &xy_test[0], &xy_test[1]);
  xy_to_uv(xy_test, uv_test);
  const float delta_daylight = sqrtf((uv_test[0] - uv_ref[0]) * (uv_test[0] - uv_ref[0]) + (uv_test[1] - uv_ref[1]) * (uv_test[1] - uv_ref[1]));
  daylight_or_bb(1, t, &xy_test[0], &xy_test[1]);
  xy_to_uv(xy_test, uv_test);
  const float delta_bb = sqrtf((uv_test[0] - uv_ref[0]) * (uv_test[0] - uv_ref[0]) + (uv_test[1] - uv_ref[1]) * (uv_test[1] - uv_ref[1]));
  if(delta_bb < 0.005f || delta_daylight < 0.005f)
    return delta_bb < delta_daylight ? "bb" : "d";
  return "custom";
}

/* dt_colorspaces_conversion_matrices_rgb's multipliers, green normalized */
static void d65_coeffs(const float XYZ_to_CAM[4][3], double bwb[3])
{
  static const double RGB_to_XYZ[3][3] = {
    { 0.412453, 0.357580, 0.180423 }, { 0.212671, 0.715160, 0.072169 }, { 0.019334, 0.119193, 0.950227 } };
  double mul[3];
  for(int i = 0; i < 3; i++)
  {
    double num = 0.0;
    for(int j = 0; j < 3; j++) { double v = 0.0; for(int k = 0; k < 3; k++) v += XYZ_to_CAM[i][k] * RGB_to_XYZ[k][j]; num += v; }
    mul[i] = 1.0 / num;
  }
  bwb[0] = mul[0] / mul[1]; bwb[1] = 1.0; bwb[2] = mul[2] / mul[1];
}

int main(void)
{
  struct { const char *name; float m[9]; } cams[] = {
    { "Nikon D750", { 9020,-2890,-715,-4535,12436,2348,-934,1919,7086 } },
    { "Canon EOS 5D Mark IV", { 6446,-366,-864,-4436,12204,2513,-952,2496,6348 } },
    { "Sony ILCE-7M3", { 7374,-2389,-551,-5435,13162,2519,-1006,1795,6552 } },
    { "Fujifilm X-T3", { 13426,-6334,-1177,-4244,12136,2371,-580,1303,5980 } },
  };
  /* as-shot coefficients: daylight, tungsten, shade, a raw-scale daylight,
     a green cast, and one with a zero */
  const float wbs[][3] = {
    { 2.05f, 1.0f, 1.38f }, { 1.32f, 1.0f, 2.45f }, { 2.45f, 1.0f, 1.16f },
    { 2098.0f, 1024.0f, 1460.0f }, { 1.2f, 1.0f, 1.1f }, { 0.0f, 1.0f, 1.5f },
  };
  const size_t ncams = sizeof(cams)/sizeof(cams[0]), nwbs = sizeof(wbs)/sizeof(wbs[0]);
  printf("// Generated by `dt_as_shot.c` (gcc -O2 dt_as_shot.c -lm && ./a.out): darktable's\n");
  printf("// own code run on these cameras and as-shot coefficients. Do not edit.\n");
  printf("#![allow(clippy::excessive_precision, clippy::struct_field_names)]\n\n");
  printf("pub struct Camera {\n    pub name: &'static str,\n    pub xyz_to_camera: [f64; 9],\n    pub d65: [f64; 3],\n    pub shots: [Shot; %zu],\n}\n\n", nwbs + 1);
  printf("pub struct Shot {\n    pub wb: [f64; 3],\n    /// `None` where darktable finds no illuminant.\n    pub xy: Option<[f32; 2]>,\n    pub temperature: f32,\n    pub kind: &'static str,\n}\n\n");
  printf("pub const CAMERAS: [Camera; %zu] = [\n", ncams);
  for(size_t c = 0; c < ncams; c++)
  {
    float XYZ_to_CAM[4][3] = { { 0 } };
    for(int i = 0; i < 9; i++) XYZ_to_CAM[i / 3][i % 3] = cams[c].m[i] / 10000.0f;
    double d65[3];
    d65_coeffs(XYZ_to_CAM, d65);
    printf("    Camera {\n        name: \"%s\",\n        xyz_to_camera: [", cams[c].name);
    for(int i = 0; i < 9; i++) printf("%.9e%s", XYZ_to_CAM[i / 3][i % 3], i < 8 ? ", " : "");
    printf("],\n        d65: [%.12e, %.12e, %.12e],\n        shots: [\n", d65[0], d65[1], d65[2]);
    /* the given shots, then the camera's own D65 coefficients */
    for(size_t w = 0; w <= nwbs; w++)
    {
      const float d65f[3] = { (float)d65[0], 1.0f, (float)d65[2] };
      const float *wb = w < nwbs ? wbs[w] : d65f;
      float x = 0.f, y = 0.f, t = 0.f;
      const char *kind = "none";
      const gboolean found = as_shot_xy(XYZ_to_CAM, wb, &x, &y);
      if(found) kind = close_to_daylight(x, y, &t);
      printf("            Shot {\n                wb: [%.9e, %.9e, %.9e],\n", wb[0], wb[1], wb[2]);
      if(found) printf("                xy: Some([%.9e, %.9e]),\n", x, y);
      else printf("                xy: None,\n");
      printf("                temperature: %.9e,\n                kind: \"%s\",\n            },\n", t, kind);
    }
    printf("        ],\n    },\n");
  }
  printf("];\n");
  return 0;
}
