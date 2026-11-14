//! One camera look from several photos of the same camera
//! (`docs/design/camera-look.md`, "Built: a selection and a default per
//! camera"): each photo is fitted on its own, with its own gates, then
//! the looks that passed are averaged into one, and that look is scored
//! on every photo's held-out pairs together.
//!
//! - **Colour half:** the matrices, the hue/saturation tables and the
//!   chroma curves of the looks with colour (not tone-only) are averaged
//!   node by node; a look without a table or a chroma curve counts as the
//!   identity there, so a stage only some photos earned is damped, not
//!   dropped.
//! - **Tone half:** the tone curves are averaged at every knot any of
//!   them has (at most [`MAX_KNOTS`], evenly picked), each curve read with
//!   its shoulder past its last knot. An average of non-decreasing curves
//!   is non-decreasing, so the curve stays valid.
//! - **Score:** the averaged look, brought back to each photo's fit space
//!   (its exposure gain taken off the tone curve, as
//!   [`super::render::for_chain`] put it on), against each photo's camera
//!   JPEG on that photo's held-out pairs; the errors of every photo are
//!   pooled into one mean and one 95th percentile, and so are the default
//!   render's.
//!
//! The looks are in the chain's units ([`super::render::for_chain`]):
//! averaging them is averaging what the chain renders.

use helios_photo::color::{IDENTITY, Mat3};

use super::render::for_chain;
use super::{
    Attempt, CHROMA_NODES, ChromaCurve, FITTER_VERSION, HueSatTable, Kind, Look, LookInputs,
    Report, ToneCurve, apply, errors, pairs, score, split,
};

/// The averaged tone curve's knots, at most.
pub const MAX_KNOTS: usize = 48;

/// A photo's fitted look, as the chain runs it, with what scoring it
/// again needs: the exposure gain between the two halves and the fit's
/// inputs.
pub struct Fitted<'a> {
    pub look: &'a Look,
    pub gain: f64,
    pub inputs: LookInputs<'a>,
}

/// The one look of `fitted` (none for none), scored on their held-out
/// pairs together.
pub fn combine(fitted: &[Fitted<'_>]) -> Option<Look> {
    let first = fitted.first()?;
    let looks: Vec<&Look> = fitted.iter().map(|f| f.look).collect();
    let coloured: Vec<&Look> = looks
        .iter()
        .copied()
        .filter(|l| l.kind != Kind::ToneOnly)
        .collect();
    let kind = if coloured.is_empty() {
        Kind::ToneOnly
    } else if coloured.iter().all(|l| l.kind == Kind::Full) {
        Kind::Full
    } else {
        Kind::Partial
    };
    let mut look = Look {
        version: FITTER_VERSION,
        kind,
        matrix: mean_matrix(&coloured),
        table: mean_table(&coloured),
        tone: mean_tone(&looks),
        chroma: mean_chroma(&coloured),
        report: first.look.report.clone(),
    };
    let lightness = kind == Kind::ToneOnly;
    let (mut with, mut without) = (Vec::new(), Vec::new());
    for f in fitted {
        let inputs = &f.inputs;
        let held = split(inputs.sensor.width, inputs.sensor.height);
        let p = pairs(inputs, &held, None);
        let fit_space = for_chain(&look, 1.0 / f.gain);
        with.extend(errors(
            |i| apply(&fit_space, inputs.sensor.rgb[i]),
            inputs.preview,
            &p.held,
            lightness,
        ));
        without.extend(errors(
            |i| inputs.baseline.rgb[i],
            inputs.preview,
            &p.held,
            lightness,
        ));
    }
    look.report = Report {
        held_out: score(&with),
        baseline: score(&without),
        pairs: looks.iter().map(|l| l.report.pairs).sum(),
        held_out_pairs: with.len(),
        attempt: Attempt::AllPixels,
        notes: Vec::new(),
        refined: false,
    };
    Some(look)
}

fn mean_matrix(looks: &[&Look]) -> Mat3 {
    if looks.is_empty() {
        return IDENTITY;
    }
    let n = looks.len() as f64;
    std::array::from_fn(|r| {
        std::array::from_fn(|c| looks.iter().map(|l| l.matrix[r][c]).sum::<f64>() / n)
    })
}

fn mean_table(looks: &[&Look]) -> Option<HueSatTable> {
    let some = looks.iter().find_map(|l| l.table.as_ref())?;
    let identity = HueSatTable::identity(some.neutral);
    let n = looks.len() as f64;
    let mut out = HueSatTable {
        neutral: some.neutral,
        nodes: vec![[0.0; 3]; identity.nodes.len()],
    };
    for l in looks {
        let t = l
            .table
            .as_ref()
            .filter(|t| t.nodes.len() == identity.nodes.len())
            .unwrap_or(&identity);
        for (o, v) in out.nodes.iter_mut().zip(&t.nodes) {
            for (o, v) in o.iter_mut().zip(v) {
                *o += v / n;
            }
        }
    }
    Some(out)
}

fn mean_chroma(looks: &[&Look]) -> Option<ChromaCurve> {
    looks.iter().find_map(|l| l.chroma.as_ref())?;
    let n = looks.len() as f64;
    let mut nodes = [0.0; CHROMA_NODES];
    for l in looks {
        let c = l.chroma.as_ref().map_or([1.0; CHROMA_NODES], |c| c.nodes);
        for (o, v) in nodes.iter_mut().zip(c) {
            *o += v / n;
        }
    }
    Some(ChromaCurve { nodes })
}

fn mean_tone(looks: &[&Look]) -> ToneCurve {
    let mut scene: Vec<f64> = looks
        .iter()
        .flat_map(|l| l.tone.knots.iter().map(|k| k[0]))
        .filter(|s| s.is_finite() && *s >= 0.0)
        .collect();
    scene.sort_by(f64::total_cmp);
    scene.dedup_by(|a, b| (*a - *b).abs() <= 1e-9 * b.abs().max(1e-9));
    if scene.len() > MAX_KNOTS {
        let last = scene.len() - 1;
        scene = (0..MAX_KNOTS)
            .map(|i| scene[i * last / (MAX_KNOTS - 1)])
            .collect();
    }
    let n = looks.len() as f64;
    ToneCurve {
        knots: scene
            .into_iter()
            .map(|s| [s, looks.iter().map(|l| l.tone.eval(s)).sum::<f64>() / n])
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Proxy, Score, Source};
    use super::*;

    fn look(kind: Kind, gain: f64, knots: Vec<[f64; 2]>, chroma: Option<f64>) -> Look {
        let score = Score {
            mean: 1.0,
            p95: 2.0,
        };
        Look {
            version: FITTER_VERSION,
            kind,
            matrix: if kind == Kind::ToneOnly {
                IDENTITY
            } else {
                [[1.0 + gain, -gain, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            },
            table: None,
            tone: ToneCurve { knots },
            chroma: chroma.map(|c| ChromaCurve {
                nodes: [c; CHROMA_NODES],
            }),
            report: Report {
                held_out: score,
                baseline: score,
                pairs: 300,
                held_out_pairs: 100,
                attempt: Attempt::AllPixels,
                notes: Vec::new(),
                refined: false,
            },
        }
    }

    #[test]
    fn the_stages_are_averaged_and_a_missing_one_counts_as_the_identity() {
        let a = look(
            Kind::Full,
            0.2,
            vec![[0.0, 0.0], [0.1, 0.2], [0.5, 0.8]],
            Some(1.2),
        );
        let b = look(Kind::Partial, 0.0, vec![[0.0, 0.0], [0.2, 0.3]], None);
        let c = look(Kind::ToneOnly, 0.0, vec![[0.0, 0.0], [0.5, 0.6]], None);
        let refs = [&a, &b, &c];
        let coloured: Vec<&Look> = refs[..2].to_vec();
        let m = mean_matrix(&coloured);
        assert!((m[0][0] - 1.1).abs() < 1e-12 && (m[0][1] + 0.1).abs() < 1e-12);
        let chroma = mean_chroma(&coloured).unwrap();
        assert!((chroma.nodes[0] - 1.1).abs() < 1e-12);
        assert!(mean_table(&coloured).is_none());
        let tone = mean_tone(&refs);
        let scenes: Vec<f64> = tone.knots.iter().map(|k| k[0]).collect();
        assert_eq!(scenes, vec![0.0, 0.1, 0.2, 0.5]);
        // Every curve read at 0.1: a's knot, b and c interpolated.
        let at = (0.2 + 0.15 + 0.12) / 3.0;
        assert!((tone.knots[1][1] - at).abs() < 1e-12, "{tone:?}");
        assert!(tone.knots.windows(2).all(|w| w[1][1] >= w[0][1]));
    }

    #[test]
    fn many_knots_are_thinned_with_both_ends_kept() {
        let knots: Vec<[f64; 2]> = (0..40)
            .map(|i| [f64::from(i), f64::from(i) / 40.0])
            .collect();
        let shifted: Vec<[f64; 2]> = knots.iter().map(|k| [k[0] + 0.5, k[1]]).collect();
        let (a, b) = (
            look(Kind::ToneOnly, 0.0, knots, None),
            look(Kind::ToneOnly, 0.0, shifted, None),
        );
        let tone = mean_tone(&[&a, &b]);
        assert_eq!(tone.knots.len(), MAX_KNOTS);
        assert_eq!(tone.knots[0][0], 0.0);
        assert_eq!(tone.knots[MAX_KNOTS - 1][0], 39.5);
    }

    #[test]
    fn the_one_look_is_scored_on_every_photo_s_held_out_pairs() {
        // Two flat grey "photos" whose JPEG is the scene itself: the
        // identity look scores zero on both, their pairs pooled.
        let grey = |v: f64| Proxy::new(64, 1, vec![[v; 3]; 64]);
        let (s1, s2) = (grey(0.3), grey(0.5));
        let clipped = vec![false; 64];
        let source = Source::default();
        let inputs = |p| LookInputs {
            sensor: p,
            preview: p,
            baseline: p,
            clipped: &clipped,
            preview_clipped: &clipped,
            neutral: [1.0; 3],
            source: &source,
        };
        let identity = look(Kind::ToneOnly, 0.0, vec![[0.0, 0.0], [1.0, 1.0]], None);
        let fitted = [&s1, &s2].map(|p| Fitted {
            look: &identity,
            gain: 1.0,
            inputs: inputs(p),
        });
        let one = combine(&fitted).unwrap();
        assert_eq!(one.kind, Kind::ToneOnly);
        assert!(one.report.held_out.mean < 1e-9, "{:?}", one.report);
        assert_eq!(one.report.pairs, 600);
        let per_photo = pairs(&fitted[0].inputs, &split(64, 1), None).held.len();
        assert!(per_photo > 0);
        assert_eq!(one.report.held_out_pairs, 2 * per_photo);
        assert!(combine(&[]).is_none());
    }
}
