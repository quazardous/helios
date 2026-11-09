//! The controls gallery, native or in a browser (`trunk build` in this
//! crate's folder, as egui-pop's gallery).

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let native = eframe::NativeOptions {
        viewport: egui_pop::egui::ViewportBuilder::default()
            .with_title(helios_controls_gallery::TITLE)
            .with_inner_size([1000.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        helios_controls_gallery::TITLE,
        native,
        Box::new(|_| Ok(Box::<helios_controls_gallery::App>::default())),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    wasm_bindgen_futures::spawn_local(async {
        let Some(canvas) = eframe::web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("the_canvas_id"))
            .and_then(|e| e.dyn_into::<eframe::web_sys::HtmlCanvasElement>().ok())
        else {
            return;
        };
        let _ = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|_| Ok(Box::<helios_controls_gallery::App>::default())),
            )
            .await;
    });
}
