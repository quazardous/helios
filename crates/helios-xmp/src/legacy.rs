//! Params of older module versions, brought to the current version as
//! darktable's own `legacy_params` does it, step by step and byte for byte.
//!
//! Only the modules whose values Helios reads are migrated (white balance,
//! highlights, exposure, sigmoid); the others are taken for their presence.
//! Each step reproduces darktable's, including its quirks: its highlights
//! v3 to v4 step copies the struct as bytes, so v3's `strength` lands in
//! v4's `clip`. Tests compare every step with darktable's own conversion of
//! the old params found in darktable's test sidecars.

fn f32_at(b: &[u8], at: usize) -> Result<f32, String> {
    b.get(at..at + 4)
        .map(|s| f32::from_le_bytes(s.try_into().expect("4 bytes")))
        .ok_or_else(|| format!("old params end at {} bytes, need {}", b.len(), at + 4))
}

fn i32_at(b: &[u8], at: usize) -> Result<i32, String> {
    b.get(at..at + 4)
        .map(|s| i32::from_le_bytes(s.try_into().expect("4 bytes")))
        .ok_or_else(|| format!("old params end at {} bytes, need {}", b.len(), at + 4))
}

/// Little-endian 32-bit words.
fn words(values: &[[u8; 4]]) -> Vec<u8> {
    values.concat()
}

fn f(v: f32) -> [u8; 4] {
    v.to_le_bytes()
}

fn i(v: i32) -> [u8; 4] {
    v.to_le_bytes()
}

fn exact(b: &[u8], size: usize, what: &str) -> Result<(), String> {
    if b.len() == size {
        Ok(())
    } else {
        Err(format!("{what}: {} bytes, expected {size}", b.len()))
    }
}

/// One step: params at `version` to the next version darktable converts
/// them to. `None` when darktable has no step from there.
fn step(module: &str, version: u32, b: &[u8]) -> Option<Result<(u32, Vec<u8>), String>> {
    let r = (|| -> Result<(u32, Vec<u8>), String> {
        match (module, version) {
            // temperature: {temp_out, coeffs[3]} then {red, green, blue, various}.
            ("temperature", 2) => {
                exact(b, 16, "temperature v2")?;
                let c = [f32_at(b, 4)?, f32_at(b, 8)?, f32_at(b, 12)?];
                Ok((3, words(&[f(c[0]), f(c[1]), f(c[2]), f(f32::NAN)])))
            }
            ("temperature", 3) => {
                exact(b, 16, "temperature v3")?;
                let c = [f32_at(b, 0)?, f32_at(b, 4)?, f32_at(b, 8)?];
                // various NaN, preset unknown (-1).
                Ok((4, words(&[f(c[0]), f(c[1]), f(c[2]), f(f32::NAN), i(-1)])))
            }
            // highlights v1..v3 go to v4 by copying the old struct over the
            // v4 layout {mode, blendL, blendC, strength, clip, noise_level,
            // iterations, scales, candidating, combine, recovery, solid_color}.
            ("highlights", 1 | 2 | 3) => {
                let size = match version {
                    1 => 16,
                    2 => 20,
                    _ => 44,
                };
                exact(b, size, &format!("highlights v{version}"))?;
                let mut v4 = vec![0u8; 48];
                v4[..size].copy_from_slice(b);
                let set_f =
                    |v: &mut Vec<u8>, at: usize, x: f32| v[at..at + 4].copy_from_slice(&f(x));
                let set_i =
                    |v: &mut Vec<u8>, at: usize, x: i32| v[at..at + 4].copy_from_slice(&i(x));
                if version == 1 {
                    set_f(&mut v4, 16, 1.0); // clip
                }
                if version <= 2 {
                    set_f(&mut v4, 20, 0.0); // noise_level
                    set_i(&mut v4, 24, 1); // iterations
                    set_i(&mut v4, 28, 5); // scales
                    set_f(&mut v4, 32, 0.4); // candidating
                    set_f(&mut v4, 36, 2.0); // combine
                    set_i(&mut v4, 40, 0); // recovery: off
                }
                set_f(&mut v4, 12, 0.0); // strength
                set_f(&mut v4, 44, 0.0); // solid_color
                Ok((4, v4))
            }
            ("highlights", 4) => {
                exact(b, 48, "highlights v4")?;
                // v5 drops blendL and blendC: {mode, strength, clip, ...}.
                let mut v5 = Vec::with_capacity(40);
                v5.extend_from_slice(&b[0..4]);
                v5.extend_from_slice(&b[12..48]);
                Ok((5, v5))
            }
            // exposure: v2..v5 go to v6 {mode, black, exposure,
            // deflicker_percentile, deflicker_target_level, compensate_bias}.
            ("exposure", 2) => {
                exact(b, 12, "exposure v2")?;
                let (black, exposure) = (f32_at(b, 0)?, f32_at(b, 4)?);
                Ok((
                    6,
                    words(&[i(0), f(black), f(exposure), f(50.0), f(-4.0), i(0)]),
                ))
            }
            ("exposure", 3) => {
                exact(b, 20, "exposure v3")?;
                let mode = i32::from(i32_at(b, 8)? != 0); // deflicker: mode 1
                Ok((
                    6,
                    words(&[
                        i(mode),
                        f(f32_at(b, 0)?),
                        f(f32_at(b, 4)?),
                        f(f32_at(b, 12)?),
                        f(f32_at(b, 16)?),
                        i(0),
                    ]),
                ))
            }
            ("exposure", 4 | 5) => {
                exact(
                    b,
                    if version == 4 { 24 } else { 20 },
                    &format!("exposure v{version}"),
                )?;
                let mut v6 = b[..20].to_vec();
                v6.extend_from_slice(&i(0));
                Ok((6, v6))
            }
            ("exposure", 6) => {
                exact(b, 24, "exposure v6")?;
                let mut v7 = b.to_vec();
                v7.extend_from_slice(&i(0)); // compensate_hilite_pres: off
                Ok((7, v7))
            }
            // sigmoid: the common part copied, the rest zeroed.
            ("sigmoid", 1 | 2) => {
                let size = if version == 1 { 24 } else { 52 };
                exact(b, size, &format!("sigmoid v{version}"))?;
                let mut v3 = vec![0u8; 56];
                v3[..size].copy_from_slice(b);
                Ok((3, v3))
            }
            _ => Err(String::new()),
        }
    })();
    match r {
        Err(e) if e.is_empty() => None,
        r => Some(r),
    }
}

/// Brings params from `version` to `target`, step by step. `None` when
/// there is no way from that version.
pub fn upgrade(
    module: &str,
    version: u32,
    params: &[u8],
    target: u32,
) -> Option<Result<Vec<u8>, String>> {
    let (mut version, mut bytes) = (version, params.to_vec());
    while version < target {
        match step(module, version, &bytes)? {
            Ok((next, out)) => {
                version = next;
                bytes = out;
            }
            Err(e) => return Some(Err(format!("`{module}`: {e}"))),
        }
    }
    (version == target).then_some(Ok(bytes))
}

/// Modules Helios takes for their presence only: their params are not read
/// (the camera's description or Helios's defaults give the values), so any
/// version will do.
pub fn presence_only(module: &str) -> bool {
    matches!(module, "rawprepare" | "demosaic" | "colorin")
}
