//! Value bricks: pure functions of numbers, evaluated before any pixel.
//!
//! They are what darktable's `commit_params` does in code: exposure's
//! scale from its EV and black level, written as a small graph an agent can
//! read and a schema can check. Each declares its arithmetic: most work in
//! double precision, the ones whose result feeds pixels say when they work
//! in single precision, the precision of pixels, as the sealed modules they
//! are extracted from.

use helios_core::{BrickFamily, BrickPort, BrickSpec, PortType, Shape, Tolerance, Value};

/// Reads input `i` of a value brick, evaluated on demand: a brick reads
/// only the inputs it needs (a selection reads one branch).
pub type Input<'a> = dyn FnMut(usize) -> Result<Value, String> + 'a;

/// A value brick.
pub trait ValueBrick: Send + Sync {
    /// Its declaration: shape [`Shape::Value`], scalar or bool ports, one
    /// output.
    fn spec(&self) -> BrickSpec;

    /// Its output from its inputs, read through `input` by position in
    /// [`BrickSpec::inputs`].
    fn eval(&self, input: &mut Input<'_>) -> Result<Value, String>;
}

/// Input `i` as a number.
pub fn number(input: &mut Input<'_>, i: usize) -> Result<f64, String> {
    let v = input(i)?;
    v.as_f64()
        .ok_or_else(|| format!("input {i}: a number expected, {v:?} given"))
}

/// Input `i` as a flag.
pub fn flag(input: &mut Input<'_>, i: usize) -> Result<bool, String> {
    let v = input(i)?;
    v.as_bool()
        .ok_or_else(|| format!("input {i}: true or false expected, {v:?} given"))
}

type Eval = fn(&mut Input<'_>) -> Result<Value, String>;

/// A value brick written as a function and its description.
struct Func {
    id: &'static str,
    purpose: &'static str,
    description: &'static str,
    use_when: &'static str,
    inputs: &'static [(&'static str, bool, &'static str)],
    output: (bool, &'static str),
    eval: Eval,
}

fn port_type(flag: bool) -> PortType {
    if flag {
        PortType::Bool
    } else {
        PortType::Scalar
    }
}

impl ValueBrick for Func {
    fn spec(&self) -> BrickSpec {
        BrickSpec {
            id: self.id.into(),
            major: 1,
            family: BrickFamily::Value,
            shape: Shape::Value,
            tolerance: Tolerance::Bitwise,
            purpose: self.purpose.into(),
            description: self.description.into(),
            use_when: self.use_when.into(),
            avoid_when: "image work: value bricks never see a pixel".into(),
            keywords: vec!["value".into(), "derivation".into()],
            provenance: "Helios".into(),
            inputs: self
                .inputs
                .iter()
                .map(|(name, flag, d)| BrickPort::new(name, port_type(*flag), d))
                .collect(),
            outputs: vec![BrickPort::new(
                "output",
                port_type(self.output.0),
                self.output.1,
            )],
        }
    }

    fn eval(&self, input: &mut Input<'_>) -> Result<Value, String> {
        (self.eval)(input)
    }
}

const X: (&str, bool, &str) = ("x", false, "the number");
const SCALAR: (bool, &str) = (false, "the result");

/// The core value bricks.
pub fn core() -> Vec<Box<dyn ValueBrick>> {
    let funcs = [
        Func {
            id: "core.value.neg",
            purpose: "The opposite of a number",
            description: "-x, exact.",
            use_when: "a parameter acts the other way round (exposure's EV into a white level)",
            inputs: &[X],
            output: SCALAR,
            eval: |i| Ok(Value::Float(-number(i, 0)?)),
        },
        Func {
            id: "core.value.add",
            purpose: "The sum of two numbers",
            description: "a + b, in double precision.",
            use_when: "two offsets add up",
            inputs: &[("a", false, "a number"), ("b", false, "a number")],
            output: SCALAR,
            eval: |i| Ok(Value::Float(number(i, 0)? + number(i, 1)?)),
        },
        Func {
            id: "core.value.sub",
            purpose: "The difference of two numbers",
            description: "a - b, in double precision.",
            use_when: "an amount is taken back (a camera's exposure bias)",
            inputs: &[
                ("a", false, "the number"),
                ("b", false, "what is taken off"),
            ],
            output: SCALAR,
            eval: |i| Ok(Value::Float(number(i, 0)? - number(i, 1)?)),
        },
        Func {
            id: "core.value.mul",
            purpose: "The product of two numbers",
            description: "a * b, in double precision.",
            use_when: "a factor scales a parameter",
            inputs: &[("a", false, "a number"), ("b", false, "a number")],
            output: SCALAR,
            eval: |i| Ok(Value::Float(number(i, 0)? * number(i, 1)?)),
        },
        Func {
            id: "core.value.exp2",
            purpose: "Two to the power of a number, in single precision",
            description: "2^x computed in single precision (darktable's exp2f), the result \
                          widened back: what feeds pixels as darktable computes it.",
            use_when: "stops (EV) into a linear factor",
            inputs: &[("x", false, "the power, in stops")],
            output: (false, "2^x, a single-precision number"),
            eval: |i| Ok(Value::Float(f64::from((number(i, 0)? as f32).exp2()))),
        },
        Func {
            id: "core.value.log2",
            purpose: "The base-two logarithm of a number",
            description: "log2(x), in double precision; not a number below zero.",
            use_when: "a linear factor into stops (EV)",
            inputs: &[("x", false, "a positive number")],
            output: SCALAR,
            eval: |i| Ok(Value::Float(number(i, 0)?.log2())),
        },
        Func {
            id: "core.value.clamp",
            purpose: "A number kept between two limits",
            description: "min(max(x, min), max), in double precision.",
            use_when: "a derived value must stay in a range",
            inputs: &[
                X,
                ("min", false, "the low limit"),
                ("max", false, "the high limit"),
            ],
            output: SCALAR,
            eval: |i| {
                let (x, lo, hi) = (number(i, 0)?, number(i, 1)?, number(i, 2)?);
                Ok(Value::Float(x.max(lo).min(hi)))
            },
        },
        Func {
            id: "core.value.within",
            purpose: "A number trusted only within limits",
            description: "x when it is a finite number between min and max (included), \
                          otherwise the fallback: a value out of reason is not trusted.",
            use_when: "metadata may be wrong (a camera's exposure bias beyond 5 EV)",
            inputs: &[
                ("x", false, "the number to trust"),
                ("min", false, "the lowest trusted value"),
                ("max", false, "the highest trusted value"),
                ("fallback", false, "what an untrusted value counts as"),
            ],
            output: SCALAR,
            eval: |i| {
                let (x, lo, hi) = (number(i, 0)?, number(i, 1)?, number(i, 2)?);
                if x.is_finite() && (lo..=hi).contains(&x) {
                    Ok(Value::Float(x))
                } else {
                    Ok(Value::Float(number(i, 3)?))
                }
            },
        },
        Func {
            id: "core.value.select",
            purpose: "One of two values, by a flag",
            description: "then when the flag is true, else otherwise; only the chosen branch \
                          is evaluated, so the other may need context that is absent.",
            use_when: "an option switches a derivation on (compensate the camera's bias)",
            inputs: &[
                ("when", true, "the flag"),
                ("then", false, "the value when true"),
                ("else", false, "the value when false"),
            ],
            output: SCALAR,
            eval: |i| {
                let pick = if flag(i, 0)? { 1 } else { 2 };
                Ok(Value::Float(number(i, pick)?))
            },
        },
        Func {
            id: "core.value.below",
            purpose: "A number pulled below a limit, in single precision",
            description: "x when x < limit, otherwise limit - margin, computed in single \
                          precision (darktable keeps exposure's black below its white so).",
            use_when: "a level must stay under another one (a black under a white)",
            inputs: &[
                X,
                ("limit", false, "the value it must stay under"),
                (
                    "margin",
                    false,
                    "how far under the limit it goes when pulled",
                ),
            ],
            output: (false, "a single-precision number under the limit"),
            eval: |i| {
                let x = number(i, 0)? as f32;
                let limit = number(i, 1)? as f32;
                if x >= limit {
                    let margin = number(i, 2)? as f32;
                    Ok(Value::Float(f64::from(limit - margin)))
                } else {
                    Ok(Value::Float(f64::from(x)))
                }
            },
        },
        Func {
            id: "core.value.affine-inv",
            purpose: "The scale that maps black to zero and white to one",
            description: "1 / (white - black), computed in single precision.",
            use_when: "levels or exposure: from a black and a white level to the scale of \
                       core.affine",
            inputs: &[
                ("white", false, "the level that maps to one"),
                ("black", false, "the level that maps to zero"),
            ],
            output: (false, "the scale, a single-precision number"),
            eval: |i| {
                let white = number(i, 0)? as f32;
                let black = number(i, 1)? as f32;
                Ok(Value::Float(f64::from(1.0 / (white - black))))
            },
        },
    ];
    funcs
        .into_iter()
        .map(|f| Box::new(f) as Box<dyn ValueBrick>)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, args: &[Value]) -> Result<Value, String> {
        let brick = core()
            .into_iter()
            .find(|b| b.spec().id == id)
            .unwrap_or_else(|| panic!("{id}"));
        assert_eq!(brick.spec().inputs.len(), args.len(), "{id}");
        brick.eval(&mut |k| Ok(args[k].clone()))
    }

    fn f(v: f64) -> Value {
        Value::Float(v)
    }

    #[test]
    fn each_computes_what_it_says() {
        assert_eq!(run("core.value.neg", &[f(2.0)]), Ok(f(-2.0)));
        assert_eq!(run("core.value.add", &[f(2.0), f(0.5)]), Ok(f(2.5)));
        assert_eq!(run("core.value.sub", &[f(2.0), f(0.5)]), Ok(f(1.5)));
        assert_eq!(run("core.value.mul", &[f(2.0), f(0.5)]), Ok(f(1.0)));
        assert_eq!(run("core.value.exp2", &[f(-2.0)]), Ok(f(0.25)));
        assert_eq!(run("core.value.log2", &[f(8.0)]), Ok(f(3.0)));
        assert_eq!(
            run("core.value.clamp", &[f(9.0), f(0.0), f(1.0)]),
            Ok(f(1.0))
        );
        assert_eq!(run("core.value.affine-inv", &[f(1.0), f(0.5)]), Ok(f(2.0)));
    }

    #[test]
    fn within_trusts_only_finite_values_in_range() {
        let w = |x| run("core.value.within", &[f(x), f(-5.0), f(5.0), f(0.0)]);
        assert_eq!(w(5.0), Ok(f(5.0)));
        assert_eq!(w(-1.5), Ok(f(-1.5)));
        assert_eq!(w(12.0), Ok(f(0.0)));
        assert_eq!(w(f64::NAN), Ok(f(0.0)));
        assert_eq!(w(f64::INFINITY), Ok(f(0.0)));
    }

    #[test]
    fn select_reads_only_the_chosen_branch() {
        let brick = core()
            .into_iter()
            .find(|b| b.spec().id == "core.value.select")
            .unwrap();
        let mut read = Vec::new();
        let out = brick.eval(&mut |k| {
            read.push(k);
            match k {
                0 => Ok(Value::Bool(false)),
                1 => Err("absent".into()),
                _ => Ok(f(3.0)),
            }
        });
        assert_eq!(out, Ok(f(3.0)));
        assert_eq!(read, [0, 2]);
    }

    #[test]
    fn below_pulls_in_single_precision_like_darktable() {
        // The margin as darktable writes it, a single-precision literal.
        assert_eq!(0.01f64 as f32, 0.01f32);
        let white = 0.5f32;
        let pulled = run("core.value.below", &[f(0.8), f(f64::from(white)), f(0.01)]);
        assert_eq!(pulled, Ok(f(f64::from(white - 0.01f32))));
        assert_eq!(
            run("core.value.below", &[f(0.1), f(0.5), f(0.01)]),
            Ok(f(f64::from(0.1f32)))
        );
    }

    #[test]
    fn a_wrong_input_type_is_named() {
        let err = run("core.value.neg", &[Value::Bool(true)]).unwrap_err();
        assert!(err.contains("a number expected"), "{err}");
    }
}
