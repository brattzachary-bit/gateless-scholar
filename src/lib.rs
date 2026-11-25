//! Gateless Scholar — research, ungated.
//!
//! A native app for people without university logins: Scholar-style search, a daily Discover
//! feed of research and news, and AI-written, fully cited articles with the origin of every
//! passage marked. Runs on Linux desktops (`src/main.rs`) and Android (`android_main` below).

pub mod app;
pub mod db;
pub mod export;
pub mod jobs;
pub mod model;
pub mod platform;
pub mod providers;
pub mod relay;
pub mod research;
pub mod settings;
pub mod ui;
pub mod writing;

/// One tokio runtime for all background work (research, writing, images, exports).
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("gateless-worker").build().expect("tokio runtime")
}

/// The desktop app (Linux).
#[cfg(not(target_os = "android"))]
pub fn run_desktop() -> eframe::Result<()> {
    use craft_window::Prefix;
    use craft_window::policy::{Backend, TitleBarPref, apply, decide_for};

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,gateless=info")).init();
    let db = match platform::data_dir().and_then(|d| db::Db::open(&d)) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Gateless Scholar could not open its database: {e:#}");
            std::process::exit(1);
        }
    };
    let settings = db.load_settings();
    let runtime = runtime();
    let handle = runtime.handle().clone();

    // Who draws the title bar: the app's own (default), or the desktop's.
    // TODO(windows): craft-window decides Windows captions; check the integrated bar there.
    let pref = if settings.system_title_bar { TitleBarPref::System } else { TitleBarPref::Integrated };
    let decision = decide_for(Prefix::new("GATELESS"), Backend::from_env(), pref);
    let viewport = apply(
        egui::ViewportBuilder::default()
            .with_title("Gateless Scholar")
            .with_app_id("org.gatelessscholar.Gateless")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 520.0])
            .with_icon(app_icon()),
        &decision.frame,
    );
    let frame = decision.frame;
    let options = eframe::NativeOptions { viewport, persist_window: true, ..Default::default() };
    let result = eframe::run_native("Gateless Scholar", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, db, handle, frame)))));
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));
    result
}

/// The Android app: NativeActivity calls this.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: platform::android_activity::AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("gateless"));
    platform::set_android_app(app.clone());
    let db = match platform::data_dir().and_then(|d| db::Db::open(&d)) {
        Ok(db) => db,
        Err(e) => {
            log::error!("could not open the database: {e:#}");
            return;
        }
    };
    let runtime = runtime();
    let handle = runtime.handle().clone();
    let options = eframe::NativeOptions { android_app: Some(app), ..Default::default() };
    if let Err(e) = eframe::run_native("Gateless Scholar", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, db, handle, craft_window::policy::Frame::Native))))) {
        log::error!("eframe: {e}");
    }
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));
}

/// The window icon: an arch (the open gate) drawn in code, so no asset is needed at runtime.
#[cfg(not(target_os = "android"))]
fn app_icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = (y * n + x) * 4;
            let inside = {
                let r = 12.0;
                let cx = fx.clamp(r, n as f32 - r);
                let cy = fy.clamp(r, n as f32 - r);
                (fx - cx).powi(2) + (fy - cy).powi(2) <= r * r
            };
            if !inside {
                continue;
            }
            let (mut r, mut g, mut b) = (0x19u8, 0x1au8, 0x1au8);
            let d = ((fx - 32.0).powi(2) + (fy - 30.0).powi(2)).sqrt();
            let arch = (fy <= 30.0 && (13.0..=19.0).contains(&d)) || (fy > 30.0 && fy <= 50.0 && ((13.0..=19.0).contains(&(fx - 32.0).abs())));
            let ground = (50.0..=54.0).contains(&fy) && (8.0..=56.0).contains(&fx);
            if arch || ground {
                (r, g, b) = (0x20, 0xb8, 0xcd);
            }
            rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}
