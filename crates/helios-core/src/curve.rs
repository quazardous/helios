//! A piecewise-linear curve: how a macro's semantic slider moves one inner
//! parameter (see [`crate::Macro::drive`]).

/// At most this many points on a curve: few enough to stay data an AI
/// writes and a user reads, enough for a knee or two.
pub const MAX_POINTS: usize = 8;

/// Points `(x, y)` joined by straight lines, `x` strictly increasing.
/// Before its first point and after its last one the curve stays flat.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    points: Vec<(f64, f64)>,
}

impl Curve {
    /// A curve through `points`: 2 to [`MAX_POINTS`] finite points, `x`
    /// strictly increasing. Refused with the reason otherwise.
    pub fn new(points: Vec<(f64, f64)>) -> Result<Self, String> {
        if points.len() < 2 {
            return Err("a curve has at least 2 points".into());
        }
        if points.len() > MAX_POINTS {
            return Err(format!(
                "a curve has at most {MAX_POINTS} points, not {}",
                points.len()
            ));
        }
        for (i, (x, y)) in points.iter().enumerate() {
            if !(x.is_finite() && y.is_finite()) {
                return Err(format!("point {i} is not a pair of numbers"));
            }
            if i > 0 && *x <= points[i - 1].0 {
                return Err(format!(
                    "point {i}: x {x} does not follow {} (x strictly increasing)",
                    points[i - 1].0
                ));
            }
        }
        Ok(Self { points })
    }

    pub fn points(&self) -> &[(f64, f64)] {
        &self.points
    }

    /// The `x` it spans: its first and last points'.
    pub fn domain(&self) -> (f64, f64) {
        (self.points[0].0, self.points[self.points.len() - 1].0)
    }

    /// The lowest and highest `y` it takes.
    pub fn span(&self) -> (f64, f64) {
        self.points
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (_, y)| {
                (lo.min(*y), hi.max(*y))
            })
    }

    /// Its value at `x`: interpolated between the two points around it,
    /// flat outside its domain. A point's own `x` gives its `y` exactly.
    pub fn eval(&self, x: f64) -> f64 {
        let p = &self.points;
        if x.is_nan() {
            return p[0].1;
        }
        if x <= p[0].0 {
            return p[0].1;
        }
        let last = p[p.len() - 1];
        if x >= last.0 {
            return last.1;
        }
        let i = p.partition_point(|(px, _)| *px <= x);
        let (x0, y0) = p[i - 1];
        let (x1, y1) = p[i];
        if x == x0 {
            return y0;
        }
        y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    }
}

impl Curve {
    /// The `x` where a strictly monotonic curve takes `y` (within its
    /// span); None for a curve that is not, or a `y` outside its span.
    pub fn invert(&self, y: f64) -> Option<f64> {
        let p = &self.points;
        let up = p.windows(2).all(|w| w[1].1 > w[0].1);
        let down = p.windows(2).all(|w| w[1].1 < w[0].1);
        if !(up || down) || !y.is_finite() {
            return None;
        }
        p.windows(2).find_map(|w| {
            let ((x0, y0), (x1, y1)) = (w[0], w[1]);
            let (lo, hi) = (y0.min(y1), y0.max(y1));
            (lo..=hi)
                .contains(&y)
                .then(|| x0 + (x1 - x0) * (y - y0) / (y1 - y0))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(points: &[(f64, f64)]) -> Curve {
        Curve::new(points.to_vec()).unwrap()
    }

    #[test]
    fn interpolates_between_points() {
        let c = curve(&[(0.0, 0.0), (0.5, 10.0), (1.0, 30.0)]);
        assert_eq!(c.eval(0.0), 0.0);
        assert_eq!(c.eval(0.25), 5.0);
        assert_eq!(c.eval(0.5), 10.0);
        assert_eq!(c.eval(0.75), 20.0);
        assert_eq!(c.eval(1.0), 30.0);
        assert_eq!(c.domain(), (0.0, 1.0));
        assert_eq!(c.span(), (0.0, 30.0));
    }

    #[test]
    fn flat_outside_its_domain() {
        let c = curve(&[(-1.0, 2.0), (1.0, -2.0)]);
        assert_eq!(c.eval(-5.0), 2.0);
        assert_eq!(c.eval(7.0), -2.0);
        assert_eq!(c.eval(0.0), 0.0);
        assert_eq!(c.eval(f64::NAN), 2.0);
        assert_eq!(c.eval(f64::INFINITY), -2.0);
    }

    #[test]
    fn decreasing_and_non_monotonic_curves() {
        let c = curve(&[(0.0, 1.0), (0.5, 3.0), (1.0, 1.0)]);
        assert_eq!(c.eval(0.25), 2.0);
        assert_eq!(c.eval(0.75), 2.0);
        assert_eq!(c.span(), (1.0, 3.0));
    }

    #[test]
    fn inverts_a_monotonic_curve_only() {
        let c = curve(&[(0.0, 0.0), (0.5, 10.0), (1.0, 30.0)]);
        assert_eq!(c.invert(5.0), Some(0.25));
        assert_eq!(c.invert(20.0), Some(0.75));
        assert_eq!(c.invert(31.0), None);
        let down = curve(&[(-1.0, 2.0), (1.0, -2.0)]);
        assert_eq!(down.invert(1.0), Some(-0.5));
        let bump = curve(&[(0.0, 1.0), (0.5, 3.0), (1.0, 1.0)]);
        assert_eq!(bump.invert(2.0), None);
        let flat = curve(&[(0.0, 1.0), (1.0, 1.0)]);
        assert_eq!(flat.invert(1.0), None);
    }

    #[test]
    fn refusals() {
        for (points, why) in [
            (vec![(0.0, 0.0)], "at least 2"),
            (vec![(0.0, 0.0); 9], "at most 8"),
            (vec![(0.0, 0.0), (0.0, 1.0)], "strictly increasing"),
            (vec![(1.0, 0.0), (0.0, 1.0)], "strictly increasing"),
            (vec![(0.0, f64::NAN), (1.0, 1.0)], "pair of numbers"),
            (vec![(0.0, 0.0), (f64::INFINITY, 1.0)], "pair of numbers"),
        ] {
            let e = Curve::new(points).unwrap_err();
            assert!(e.contains(why), "{e}");
        }
        assert!(Curve::new((0..8).map(|i| (f64::from(i), 0.0)).collect()).is_ok());
    }
}
