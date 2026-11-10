//! Drawings for the integrated controls the implicit ones declare: a
//! curve and a colour wheel, drawn by egui-pop's editors.

use egui_pop::egui::{self, Rect, Ui};
use egui_pop::viewport::curve::CurveEditor;
use egui_pop::viewport::wheel::{ColorWheel, WheelValue};

use crate::model::{Control, ValueSpec, ValueType};
use crate::render::{Drawings, Edit};
use crate::value::Value;

/// egui-pop's curve editor for a list of points, its offset wheel for a
/// colour (hue, chroma, luminance), each where it has room (80 points
/// high at least); elsewhere the part's fallback is drawn.
#[derive(Clone, Copy, Debug, Default)]
pub struct EditorDrawings;

impl Drawings for EditorDrawings {
    fn draw(
        &mut self,
        ui: &mut Ui,
        key: &str,
        control: &Control,
        rect: Rect,
        value: &mut Value,
    ) -> Option<Vec<Edit>> {
        let ValueSpec::Other { stored } = &control.value else {
            return None;
        };
        let Value::Floats(list) = value else {
            return None;
        };
        if rect.height() < 80.0 {
            return None;
        }
        let set = |to: &[f64], done: bool| Edit::Set {
            key: key.to_owned(),
            to: Value::Floats(to.to_vec()),
            done,
        };
        match stored {
            ValueType::Points => {
                let mut points: Vec<[f32; 2]> = list
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| [c[0] as f32, c[1] as f32])
                    .collect();
                let side = rect.width().min(rect.height());
                let out = CurveEditor::new(&mut points, &control.label)
                    .size(egui::vec2(side, side))
                    .show(ui);
                *list = points
                    .iter()
                    .flat_map(|[x, y]| [f64::from(*x), f64::from(*y)])
                    .collect();
                Some(out.changes.iter().map(|c| set(list, c.done)).collect())
            }
            ValueType::Colour => {
                let at = |i: usize| list.get(i).copied().unwrap_or(0.0) as f32;
                let mut w = WheelValue {
                    hue: (at(0) * 360.0).rem_euclid(360.0),
                    chroma: at(1),
                    luminance: at(2),
                };
                let side = (rect.height() - 28.0).min(rect.width()).max(40.0);
                let out = ColorWheel::offset(&mut w, &control.label)
                    .size(side)
                    .show(ui);
                *list = vec![
                    f64::from(w.hue) / 360.0,
                    f64::from(w.chroma),
                    f64::from(w.luminance),
                ];
                Some(out.changes.iter().map(|c| set(list, c.done)).collect())
            }
            _ => None,
        }
    }
}
