//! The process entry point.
//!
//! Thin on purpose: parse the command line, build the window, run. Everything the
//! window does lives in the library, where it can be tested.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = mangle_ui::options_from(&args);

    // The reference window is 1536x1024 logical.
    let (mut w, mut h) = (1536.0_f32, 1024.0_f32);
    if let Some(size) = options.window_size {
        w = size.0;
        h = size.1;
    }
    let scale = if options.scale > 0.0 {
        options.scale
    } else {
        1.0
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ManglePDF")
            .with_inner_size([w, h])
            .with_min_inner_size([640.0, 480.0]),
        ..Default::default()
    };
    // The scale factor is a windowing property, not a viewport one: it changes how many
    // physical pixels a logical point covers, which is what HiDPI means.
    let _ = scale;

    // A file named on the command line is opened before the first frame, so `manglepdf a.pdf`
    // shows `a.pdf` rather than an empty window that then fills in.
    let open = options.open.clone();
    eframe::run_native(
        "ManglePDF",
        native_options,
        Box::new(move |cc| {
            let mut app = mangle_ui::App::new(cc);
            if let Some(path) = open.clone() {
                app.open(path);
            }
            Ok(Box::new(app))
        }),
    )
}
