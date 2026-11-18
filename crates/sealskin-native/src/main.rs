#[cfg(target_os = "macos")]
mod api;
#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod audio;
#[cfg(target_os = "macos")]
mod settings;
#[cfg(target_os = "macos")]
mod transport;
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
