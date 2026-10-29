//! Rustle: a native YouTube Music client.

#[cfg(not(feature = "sysalloc"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod app;
mod art_color;
mod auth;
mod backend;
mod control;
mod fallback_fonts;
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
    disable_thp();
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
            .with_min_inner_size([780.0, 480.0])
            // Without an icon, eframe decodes its own 512 px logo and keeps
            // the 1 MiB of pixels for the whole run, only to set the title
            // bar icon on Windows and macOS. On Linux the icon comes from
            // the desktop file (`with_app_id`); an empty one sets nothing.
            .with_icon(std::sync::Arc::new(egui::IconData::default())),
        // Its stand-in only logs; `fallback_fonts` finds the system fonts.
        system_font_fallback: false,
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

/// Turns off transparent huge pages for this process. With THP set to
/// "always" (Nobara's default), the kernel backs mimalloc's 2 MiB-aligned
/// arenas with huge pages, so a few KB in use cost 2 MiB of RSS. Measured
/// with `perf-check.sh --large`: peak RSS 165.9 MiB with THP, 140.8 without.
fn disable_thp() {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::{c_int, c_ulong};
        unsafe extern "C" {
            fn prctl(option: c_int, ...) -> c_int;
        }
        const PR_SET_THP_DISABLE: c_int = 41;
        // SAFETY: PR_SET_THP_DISABLE takes four unsigned long arguments and
        // only sets a flag on this process; on failure THP stays on.
        unsafe {
            prctl(
                PR_SET_THP_DISABLE,
                1 as c_ulong,
                0 as c_ulong,
                0 as c_ulong,
                0 as c_ulong,
            )
        };
    }
}
