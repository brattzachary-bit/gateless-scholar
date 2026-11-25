//! Gateless Scholar desktop binary. The app itself is the `gateless` library.

#[cfg(not(target_os = "android"))]
fn main() -> eframe::Result<()> {
    gateless::run_desktop()
}

/// On Android the app starts from `android_main` in the library; this binary is unused.
#[cfg(target_os = "android")]
fn main() {}
