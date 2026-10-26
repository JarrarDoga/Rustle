//! Rustle: a native YouTube Music client.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod app;
mod backend;
mod playback;
mod queue;
mod state;
mod theme;
mod views;
mod widgets;

fn main() -> eframe::Result {
    let demo = std::env::args().any(|a| a == "--demo");
    let perf = std::env::var_os("RUSTLE_PERF").is_some_and(|v| v == "1");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("rustle-rt")
        .build()
        .expect("tokio runtime");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rustle")
            .with_app_id("rustle")
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([780.0, 480.0]),
        ..Default::default()
    };
    let handle = rt.handle().clone();
    eframe::run_native(
        "Rustle",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::RustleApp::new(
                cc,
                demo.then_some(&handle),
                perf,
            )))
        }),
    )
}
