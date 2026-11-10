//! The `egui_hud` demo, in a browser (`trunk serve` in this crate's folder)
//! or natively (`cargo run -p egui_hud_demo`).

use egui_pop::eframe;

/// The eframe application around the demo.
struct DemoApp(egui_hud_demo::Demo);

impl eframe::App for DemoApp {
    fn ui(&mut self, ui: &mut egui_pop::egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }
}

fn create(_cc: &eframe::CreationContext<'_>) -> DemoApp {
    DemoApp(egui_hud_demo::Demo::new())
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let native = eframe::NativeOptions {
        viewport: egui_pop::egui::ViewportBuilder::default()
            .with_title(egui_hud_demo::TITLE)
            .with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        egui_hud_demo::TITLE,
        native,
        Box::new(|cc| Ok(Box::new(create(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    // A panic aborts the module: say so in the page, not with a black
    // canvas.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        failed(&format!("it panicked: {info}"));
        previous(info);
    }));

    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = eframe::web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let Some(canvas) = document
            .get_element_by_id("the_canvas_id")
            .and_then(|e| e.dyn_into::<eframe::web_sys::HtmlCanvasElement>().ok())
        else {
            failed("the page has no canvas `the_canvas_id`");
            return;
        };
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions {
                    should_prevent_default: Box::new(egui_pop::core::pointer::web_prevent_default),
                    ..eframe::WebOptions::default()
                },
                Box::new(|cc| Ok(Box::new(create(cc)))),
            )
            .await;
        if let Err(e) = started {
            failed(&format!("it did not start: {e:?}"));
        }
    });
}

/// Shows in the page, in place of the canvas, why the demo stopped.
#[cfg(target_arch = "wasm32")]
fn failed(why: &str) {
    let Some(document) = eframe::web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if let Some(canvas) = document.get_element_by_id("the_canvas_id") {
        let _ = canvas.set_attribute("hidden", "");
    }
    if let Some(message) = document.get_element_by_id("failed") {
        message.set_text_content(Some(&format!(
            "The demo could not run in this browser: {why}. The browser's console says more."
        )));
        let _ = message.remove_attribute("hidden");
    }
}
