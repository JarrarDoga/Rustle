//! Rustle: a native YouTube Music client.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod app;
mod art_color;
mod auth;
mod backend;
mod control;
mod icons;
mod media_keys;
mod playback;
mod queue;
mod settings;
mod state;
mod theme;
mod thumbs;
mod views;
mod widgets;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `rustle next` and friends: no tokio, no window.
    if let Some(code) = control::run_cli(&args) {
        return code;
    }
    let demo = args.iter().any(|a| a == "--demo");
    // With `--demo`: start with no main playlist, to show the first-run picker.
    let first_run = args.iter().any(|a| a == "--first-run");
    let perf = std::env::var_os("RUSTLE_PERF").is_some_and(|v| v == "1");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("rustle-rt")
        // Timers for the queue-save debounce; IO for rustypipe's HTTP client.
        .enable_all()
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
    let result = eframe::run_native(
        "Rustle",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::RustleApp::new(
                cc, &handle, demo, first_run, perf,
            )))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rustle: {e}");
            ExitCode::FAILURE
        }
    }
}
