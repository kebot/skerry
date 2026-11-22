#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod apps;
#[cfg(target_os = "macos")]
mod audio;
#[cfg(target_os = "macos")]
mod clipboard;
#[cfg(target_os = "macos")]
mod config_store;
#[cfg(target_os = "macos")]
mod direct;
#[cfg(target_os = "macos")]
mod gamepad;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod launch_options;
#[cfg(target_os = "macos")]
mod launch_progress;
#[cfg(target_os = "macos")]
mod open_target;
#[cfg(target_os = "macos")]
mod pasteboard;
#[cfg(target_os = "macos")]
mod persist;
#[cfg(target_os = "macos")]
mod print_dialog;
#[cfg(target_os = "macos")]
mod print_jobs;
#[cfg(target_os = "macos")]
mod sealskin;
#[cfg(target_os = "macos")]
mod selkies;
#[cfg(target_os = "macos")]
mod settings;
#[cfg(target_os = "macos")]
mod shortcuts;
mod stats;
#[cfg(target_os = "macos")]
mod tls;
#[cfg(target_os = "macos")]
mod transport;
#[cfg(target_os = "macos")]
mod video;
#[cfg(target_os = "macos")]
mod view;

#[cfg(target_os = "macos")]
fn main() {
    // The SVG icon worker (`ARCH-035`): before GPUI, Tokio, or anything else starts.
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new(apps::SVG_WORKER_FLAG)) {
        std::process::exit(apps::svg_worker_main());
    }
    app::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("SealSkin Native requires macOS.");
}
