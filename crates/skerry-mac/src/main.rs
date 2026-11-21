#[cfg(target_os = "macos")]
mod api;
#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod audio;
#[cfg(target_os = "macos")]
mod clipboard;
#[cfg(target_os = "macos")]
mod input;
#[cfg(target_os = "macos")]
mod launch_options;
#[cfg(target_os = "macos")]
mod pasteboard;
#[cfg(target_os = "macos")]
mod settings;
mod stats;
#[cfg(target_os = "macos")]
mod transport;
#[cfg(target_os = "macos")]
mod video;
#[cfg(target_os = "macos")]
mod view;

#[cfg(target_os = "macos")]
fn main() {
    app::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("SealSkin Native requires macOS.");
}
