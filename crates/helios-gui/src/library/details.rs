//! The Results as details: a row per photo, its details in columns
//! (egui-pop's data table), as a file browser's Details view. The
//! columns' sort is the gallery's (`library.sort`): a header click sorts
//! the gallery; the columns shown, their order and widths are kept with
//! the view in the workspace (`results.columns`).
//!
//! The List and Details modes open a photo on Enter or a double click and
//! carry photos out on a drag from a row, as the thumbnails do.
//!
//! upstream: egui-pop's list and table have no activation, no drag out
//! and no sort set by the application yet (filed in egui-pop's terms);
//! until they do, the rows' rects come from the cells drawn (the
//! details) or the rows' responses (the list), Enter from the focused view, and the sort goes through a value store of the
//! table's own.

use egui::{Rect, Ui};
use egui_pop::core::binding::SharedStore;
use egui_pop::core::ui::{TextRole, UiPop as _};
use egui_pop::core::widgets;
use egui_pop::model::commands::Origin;
use egui_pop::model::ids::OwnerId;
use egui_pop::model::values::{Change, Set, Store, Value};
use egui_pop::views::library::Signal;
use egui_pop::views::table::{self, Column, DataTable, Sort, TableRows};
use helios_core::Message;
use helios_template::ResultsColumn;
use helios_vault::Sort as By;

use super::{DraggedPhotos, Entry, Library};

/// Where the table's own store keeps its sort and widths.
const PREFIX: &str = "results";

/// A column the details offer.
struct Spec {
    id: &'static str,
    /// Its width at first, in points.
    width: f32,
    /// The gallery's sort a click on its header sets, when it sorts.
    sort: Option<By>,
    /// Shown until the user chooses the columns.
    shown: bool,
}

/// Every column, in the order first shown.
const COLUMNS: [Spec; 11] = [
    Spec {
        id: "name",
        width: 200.0,
        sort: Some(By::Name),
        shown: true,
    },
    Spec {
        id: "captured",
        width: 150.0,
        sort: Some(By::Captured),
        shown: true,
    },
    Spec {
        id: "rating",
        width: 80.0,
        sort: Some(By::Rating),
        shown: true,
    },
    Spec {
        id: "flag",
        width: 56.0,
        sort: None,
        shown: true,
    },
    Spec {
        id: "labels",
        width: 110.0,
        sort: None,
        shown: true,
    },
    Spec {
        id: "size",
        width: 80.0,
        sort: Some(By::Size),
        shown: true,
    },
    Spec {
        id: "camera",
        width: 150.0,
        sort: None,
        shown: true,
    },
    Spec {
        id: "lens",
        width: 150.0,
        sort: None,
        shown: false,
    },
    Spec {
        id: "exposure",
        width: 120.0,
        sort: None,
        shown: false,
    },
    Spec {
        id: "iso",
        width: 64.0,
        sort: None,
        shown: false,
    },
    Spec {
        id: "modified",
        width: 150.0,
        sort: None,
        shown: false,
    },
];

fn spec(id: &str) -> Option<&'static Spec> {
    COLUMNS.iter().find(|c| c.id == id)
}

/// A column's title.
fn title(id: &str) -> Message {
    match id {
        "name" => Message::new("gui-library-sort-name", "File name"),
        "captured" => Message::new("gui-library-sort-captured", "Capture time"),
        "rating" => Message::new("gui-library-sort-rating", "Rating"),
        "flag" => Message::new("gui-library-column-flag", "Flag"),
        "labels" => Message::new("gui-library-column-labels", "Labels"),
        "size" => Message::new("gui-library-sort-size", "File size"),
        "camera" => Message::new("gui-library-meta-camera", "Camera"),
        "lens" => Message::new("gui-library-meta-lens", "Lens"),
        "exposure" => Message::new("gui-library-meta-exposure", "Exposure"),
        "iso" => Message::new("gui-library-column-iso", "ISO"),
        _ => Message::new("gui-library-column-modified", "Modified"),
    }
}

/// The columns shown: the user's (known ones, the name always first),
/// else the default ones.
pub(super) fn shown(chosen: &[ResultsColumn]) -> Vec<ResultsColumn> {
    let mut out: Vec<ResultsColumn> = chosen
        .iter()
        .filter(|c| spec(&c.id).is_some())
        .cloned()
        .collect();
    if out.is_empty() {
        out = COLUMNS
            .iter()
            .filter(|c| c.shown)
            .map(|c| ResultsColumn {
                id: c.id.into(),
                width: None,
            })
            .collect();
    }
    out.retain(|c| c.id != "name");
    out.insert(
        0,
        chosen
            .iter()
            .find(|c| c.id == "name")
            .cloned()
            .unwrap_or(ResultsColumn {
                id: "name".into(),
                width: None,
            }),
    );
    out
}

/// The gallery's sort as the table's: on a column when one sorts that
/// way.
pub(super) fn table_sort(order: helios_vault::Order) -> Option<Sort> {
    COLUMNS
        .iter()
        .find(|c| c.sort == Some(order.by))
        .map(|c| Sort {
            column: c.id.into(),
            descending: order.descending,
        })
}

/// What a header click asks of the gallery's sort: the table's next
/// order (up, down, then "unsorted", which goes back up).
pub(super) fn gallery_sort(sort: Option<&Sort>, clicked: &str) -> Option<helios_vault::Order> {
    let (column, descending) = match sort {
        Some(s) => (s.column.as_str(), s.descending),
        None => (clicked, false),
    };
    spec(column)?
        .sort
        .map(|by| helios_vault::Order { by, descending })
}

/// The value store the table reads its sort and widths from, made for
/// one set of columns.
pub(crate) struct TableStore {
    ids: Vec<String>,
    store: SharedStore,
}

impl TableStore {
    fn new(columns: &[Column]) -> Self {
        let store: SharedStore = std::sync::Arc::new(std::sync::Mutex::new(Store::new()));
        // A fresh store: the two values are not declared yet.
        let _ = table::declare(&store, &OwnerId::FRAMEWORK, PREFIX, columns);
        TableStore {
            ids: columns.iter().map(|c| c.id.clone()).collect(),
            store,
        }
    }

    fn set_sort(&self, sort: Option<&Sort>) {
        let path = format!("{PREFIX}/sort");
        let text = Sort::encode(sort);
        let mut s = egui_pop::core::lock(&self.store);
        if matches!(s.get(&path).map(|e| &e.value), Some(Value::Text(t)) if *t == text) {
            return;
        }
        let _ = s.set(
            Set::new(&path, Change::To(Value::Text(text)), Origin::User)
                .by(&OwnerId::FRAMEWORK)
                .untracked(),
        );
    }
}

/// The photos the details show, in the gallery's order; the rows' rects
/// as drawn, for the gestures egui-pop's table does not signal yet.
struct Rows<'a> {
    entries: &'a [Entry],
    order: &'a [usize],
    columns: &'a [ResultsColumn],
    drawn: Vec<(Rect, String)>,
}

impl Rows<'_> {
    fn text(&self, row: usize, column: &str) -> String {
        let e = &self.entries[self.order[row]];
        match column {
            "name" => e.name.clone(),
            "captured" => e
                .captured
                .and_then(|s| chrono::DateTime::from_timestamp(s as i64, 0))
                .map(|d| d.naive_utc().format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default(),
            "rating" => "★".repeat(e.rating.clamp(0, 5) as usize),
            "flag" => match e.flag() {
                super::Flag::Reject => "✕".into(),
                super::Flag::Pick => "⚑".into(),
                super::Flag::None => String::new(),
            },
            "labels" => e.labels.join(", "),
            "size" => e.bytes.map(file_size).unwrap_or_default(),
            "camera" => [e.camera_make.as_deref(), e.camera_model.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" "),
            "lens" => e.lens.clone().unwrap_or_default(),
            "exposure" => {
                let mut parts = Vec::new();
                if let Some(s) = e.shutter {
                    parts.push(if s > 0.0 && s < 1.0 {
                        format!("1/{:.0} s", 1.0 / s)
                    } else {
                        format!("{s} s")
                    });
                }
                if let Some(a) = e.aperture {
                    parts.push(format!("f/{a:.1}"));
                }
                parts.join(" · ")
            }
            "iso" => e.iso.map(|i| format!("{i:.0}")).unwrap_or_default(),
            "modified" => e
                .modified
                .and_then(|s| chrono::DateTime::from_timestamp(s, 0))
                .map(|d| {
                    d.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn drawn_at(&mut self, row: usize, rect: Rect) {
        let key = &self.entries[self.order[row]].id;
        match self.drawn.iter_mut().find(|(_, k)| k == key) {
            Some((r, _)) => *r = r.union(rect),
            None => self.drawn.push((rect, key.clone())),
        }
    }
}

/// A file's size, as a file browser says it.
fn file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1000.0;
    let mut unit = 0;
    while size >= 1000.0 && unit < UNITS.len() - 1 {
        size /= 1000.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

impl TableRows<String> for Rows<'_> {
    fn len(&self) -> usize {
        self.order.len()
    }

    fn key(&self, row: usize) -> String {
        self.entries[self.order[row]].id.clone()
    }

    fn label(&self, row: usize) -> String {
        self.entries[self.order[row]].name.clone()
    }

    fn cell(&mut self, ui: &mut Ui, row: usize, column: usize, _selected: bool) {
        self.drawn_at(row, ui.max_rect());
        let Some(c) = self.columns.get(column) else {
            return;
        };
        let text = self.text(row, &c.id);
        if !text.is_empty() {
            ui.text_role(TextRole::Body, text);
        }
    }
}

/// The photos a Results list shows, in the gallery's order.
pub(super) struct ListedRows<'a> {
    pub(super) entries: &'a [Entry],
    pub(super) order: &'a [usize],
}

/// The photo of the list row at `p`, from the rows' responses.
///
/// upstream: egui-pop's list names its rows `Id::unique(("egui_pop_views::list",
/// <list id>)).with(("row", <key>))`; read until it signals activation
/// and drag out itself.
pub(super) fn list_row_at(
    ctx: &egui::Context,
    list: &str,
    keys: impl Iterator<Item = String>,
    p: egui::Pos2,
) -> Option<String> {
    let id = egui::Id::unique(("egui_pop_views::list", &list.to_string()));
    keys.into_iter().find(|key| {
        ctx.read_response(id.with(("row", key)))
            .is_some_and(|r| r.rect.contains(p))
    })
}

impl egui_pop::views::list::ListRows<String> for ListedRows<'_> {
    fn len(&self) -> usize {
        self.order.len()
    }

    fn key(&self, row: usize) -> String {
        self.entries[self.order[row]].id.clone()
    }

    fn label(&self, row: usize) -> String {
        self.entries[self.order[row]].name.clone()
    }

    fn trailing(&self, row: usize) -> Option<String> {
        let e = &self.entries[self.order[row]];
        let stars = "★".repeat(e.rating.clamp(0, 5) as usize);
        let flag = if e.rejected {
            " ✕"
        } else if e.picked {
            " ⚑"
        } else {
            ""
        };
        let text = format!("{stars}{flag}");
        (!text.is_empty()).then_some(text)
    }
}

/// What the rows' gestures ask: Enter on the focused view or a double
/// click on a row opens a photo; a drag from a row carries the selection
/// out when the row is in it, else that row's photo alone.
pub(super) fn row_gestures(
    ui: &Ui,
    response: &egui::Response,
    row_at: &dyn Fn(egui::Pos2) -> Option<String>,
    selection: &egui_pop::views::select::Selection<String>,
) -> Vec<Signal<String>> {
    let mut out = Vec::new();
    if response.has_focus()
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        && let Some(key) = selection.cursor().or(selection.keys().first())
    {
        out.push(Signal::Activate { key: key.clone() });
    }
    let (double, at) = ui.input(|i| {
        // A third click quick after a double one still opens.
        let button = egui::PointerButton::Primary;
        (
            i.pointer.button_double_clicked(button) || i.pointer.button_triple_clicked(button),
            i.pointer.interact_pos().or(i.pointer.latest_pos()),
        )
    });
    if double && let Some(key) = at.and_then(row_at) {
        out.push(Signal::Activate { key });
    }
    let (dragging, origin) = ui.input(|i| {
        (
            i.pointer.is_decidedly_dragging() && i.pointer.primary_down(),
            i.pointer.press_origin(),
        )
    });
    let carrying = egui_pop::api::bus::drag::payload::<DraggedPhotos>(ui.ctx()).is_some();
    if dragging
        && !carrying
        && let Some(key) = origin.and_then(row_at)
    {
        let keys = if selection.contains(&key) {
            selection.keys().to_vec()
        } else {
            vec![key]
        };
        out.push(Signal::DragOut { keys });
    }
    out
}

impl Library {
    /// The Results as details: the columns shown, sorted as the gallery
    /// is; a header click sorts the gallery, a resize or the columns
    /// chosen in a header's menu are kept with the view.
    pub(super) fn results_table(
        &mut self,
        ui: &mut Ui,
        label: &str,
    ) -> (egui::Response, Vec<Signal<String>>) {
        use egui_pop::views::list::Signal as ListSignal;
        let ctx = ui.ctx().clone();
        let shown = shown(&self.view.columns);
        let columns: Vec<Column> = shown
            .iter()
            .filter_map(|c| {
                let s = spec(&c.id)?;
                Some(
                    Column::new(c.id.clone(), crate::i18n::text(&ctx, title(&c.id)))
                        .width(c.width.map_or(s.width, f32::from))
                        .min_width(32.0)
                        .sortable(s.sort.is_some()),
                )
            })
            .collect();
        let ids: Vec<String> = columns.iter().map(|c| c.id.clone()).collect();
        if self.view.table_store.as_ref().map(|t| &t.ids) != Some(&ids) {
            self.view.table_store = Some(TableStore::new(&columns));
            self.view.table = egui_pop::views::table::TableState::new();
        }
        let store = self.view.table_store.as_ref().expect("made above");
        store.set_sort(table_sort(self.view.sort).as_ref());
        let selection = self.view.state.selection();
        let (keys, cursor) = (selection.keys().to_vec(), selection.cursor().cloned());
        self.view.table.set_selection(keys, cursor);
        let mut rows = Rows {
            entries: &self.entries,
            order: &self.view.order,
            columns: &shown,
            drawn: Vec::new(),
        };
        // A header's menu: the columns shown (the name always is).
        let titles: Vec<(&str, String)> = COLUMNS
            .iter()
            .filter(|c| c.id != "name")
            .map(|c| (c.id, crate::i18n::text(&ctx, title(c.id))))
            .collect();
        let shown_ids: Vec<&str> = shown.iter().map(|c| c.id.as_str()).collect();
        let mut toggled: Option<&str> = None;
        let mut table = DataTable::new("details", &columns)
            .label(label)
            .mode(egui_pop::views::Mode::Multiple)
            .store(&store.store, PREFIX)
            .header_menu(|ui, _column| {
                for (id, title) in &titles {
                    let mut on = shown_ids.contains(id);
                    if widgets::checkbox(ui, &mut on, title).changed() {
                        toggled = Some(id);
                    }
                }
            });
        if let Some(app) = &self.app {
            table = table.app(app);
        }
        let out = table.show(ui, &mut self.view.table, &mut rows);
        let drawn = std::mem::take(&mut rows.drawn);
        let mut signals = Vec::new();
        for s in out.signals {
            match s {
                ListSignal::Select { keys, cursor } => {
                    self.view.state.set_selection(keys.clone(), cursor.clone());
                    signals.push(Signal::Select { keys, cursor });
                }
                ListSignal::Vetoed { step, reason } => {
                    signals.push(Signal::Vetoed { step, reason });
                }
                // egui-pop's list says more since its signals became
                // non-exhaustive; the library reads these two.
                _ => {}
            }
        }
        if out.sorted
            && let Some(clicked) = out.header_clicked.as_deref().or(
                // From the header's menu: the column the table sorts by.
                self.view.table.sort().map(|s| s.column.as_str()),
            )
            && let Some(order) = gallery_sort(self.view.table.sort(), clicked)
            && order != self.view.sort
        {
            crate::dialogs::request(
                &ctx,
                crate::dialogs::Request::Run(
                    super::SORT_COMMAND.into(),
                    serde_json::json!({ "by": order.by.name(), "descending": order.descending }),
                    helios_shell::commands::Origin::User,
                ),
            );
        }
        let mut changed = None;
        if out.resized {
            let widths = self.view.table.widths();
            changed = Some(
                shown
                    .iter()
                    .zip(widths)
                    .map(|(c, w)| ResultsColumn {
                        id: c.id.clone(),
                        width: Some(w.round() as u16),
                    })
                    .collect::<Vec<_>>(),
            );
        }
        if let Some(id) = toggled {
            let mut cols = changed.unwrap_or_else(|| shown.clone());
            match cols.iter().position(|c| c.id == id) {
                Some(i) => {
                    cols.remove(i);
                }
                None => cols.push(ResultsColumn {
                    id: id.into(),
                    width: None,
                }),
            }
            changed = Some(cols);
        }
        if let Some(cols) = changed {
            self.view.columns = cols;
            self.view.results_changed = true;
        }
        let selection = self.view.state.selection();
        let row_at = |p: egui::Pos2| {
            drawn
                .iter()
                .find(|(r, _)| r.contains(p))
                .map(|(_, k)| k.clone())
        };
        signals.extend(row_gestures(ui, &out.response, &row_at, selection));
        (out.response, signals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_columns_start_with_the_name() {
        let ids: Vec<String> = shown(&[]).into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "name", "captured", "rating", "flag", "labels", "size", "camera"
            ]
        );
        // The user's, unknown ones dropped, the name put first.
        let chosen = [
            ResultsColumn {
                id: "iso".into(),
                width: Some(50),
            },
            ResultsColumn {
                id: "gone".into(),
                width: None,
            },
            ResultsColumn {
                id: "name".into(),
                width: Some(300),
            },
        ];
        let shown = shown(&chosen);
        assert_eq!(shown[0], chosen[2]);
        assert_eq!(shown[1], chosen[0]);
        assert_eq!(shown.len(), 2);
    }

    #[test]
    fn a_header_s_sort_is_the_gallery_s() {
        let order = helios_vault::Order {
            by: By::Rating,
            descending: true,
        };
        let sort = table_sort(order).unwrap();
        assert_eq!(sort.column, "rating");
        assert!(sort.descending);
        assert_eq!(gallery_sort(Some(&sort), "rating"), Some(order));
        // "Unsorted" goes back up on the column clicked.
        assert_eq!(
            gallery_sort(None, "name"),
            Some(helios_vault::Order {
                by: By::Name,
                descending: false
            })
        );
        // A column that does not sort leaves the gallery's sort.
        assert_eq!(gallery_sort(None, "flag"), None);
        assert_eq!(file_size(1_234_567), "1.2 MB");
        assert_eq!(file_size(999), "999 B");
    }
}
