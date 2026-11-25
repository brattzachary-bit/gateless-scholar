//! What differs between the desktop and Android: where data lives, where secrets are kept,
//! saving and sharing files, the on-screen keyboard, and the system font.
//!
//! Desktop (Linux): XDG data dir, the Secret Service keyring, portal file dialogs.
//! Android: the app's private storage, a private secrets file, MediaStore Downloads and the
//! share sheet via JNI.
// TODO(windows): the desktop branches assume Linux (fontconfig, xdg-portal dialogs).

use std::path::PathBuf;

use anyhow::Result;

pub fn is_android() -> bool {
    cfg!(target_os = "android")
}

// ---------------------------------------------------------------------------------------------
// Data directory
// ---------------------------------------------------------------------------------------------

#[cfg(target_os = "android")]
static ANDROID_APP: std::sync::OnceLock<android_activity::AndroidApp> = std::sync::OnceLock::new();

#[cfg(target_os = "android")]
pub use winit::platform::android::activity as android_activity;

#[cfg(target_os = "android")]
pub fn set_android_app(app: android_activity::AndroidApp) {
    let _ = ANDROID_APP.set(app);
}

/// Where the database, caches and (on Android) secrets are stored.
pub fn data_dir() -> Result<PathBuf> {
    #[cfg(target_os = "android")]
    {
        let app = ANDROID_APP.get().ok_or_else(|| anyhow::anyhow!("Android app not initialised"))?;
        let dir = app.internal_data_path().ok_or_else(|| anyhow::anyhow!("no internal data path"))?.join("gateless");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
    #[cfg(not(target_os = "android"))]
    {
        let dirs = directories::ProjectDirs::from("org", "GatelessScholar", "gateless").ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        let dir = dirs.data_dir().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

// ---------------------------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------------------------

/// Secrets on Android: a JSON file in the app's private storage (only this app can read it).
#[cfg(target_os = "android")]
pub mod secret_file {
    use std::collections::BTreeMap;

    use anyhow::Result;

    fn path() -> Result<std::path::PathBuf> {
        Ok(super::data_dir()?.join("secrets.json"))
    }
    fn load() -> BTreeMap<String, String> {
        path().ok().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    fn save(m: &BTreeMap<String, String>) -> Result<()> {
        let p = path()?;
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(m)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(tmp, p)?;
        Ok(())
    }
    pub fn get(id: &str) -> Result<Option<String>> {
        Ok(load().get(id).cloned())
    }
    pub fn set(id: &str, value: &str) -> Result<()> {
        let mut m = load();
        m.insert(id.to_string(), value.to_string());
        save(&m)
    }
    pub fn delete(id: &str) -> Result<()> {
        let mut m = load();
        m.remove(id);
        save(&m)
    }
}

// ---------------------------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------------------------

/// Paths of the system's regular and bold sans-serif fonts, when found.
pub fn system_sans_fonts() -> (Option<String>, Option<String>) {
    #[cfg(target_os = "android")]
    {
        let pick = |c: &[&str]| c.iter().find(|p| std::path::Path::new(p).exists()).map(|p| p.to_string());
        (
            pick(&["/system/fonts/Roboto-Regular.ttf", "/system/fonts/RobotoStatic-Regular.ttf", "/system/fonts/NotoSans-Regular.ttf"]),
            pick(&["/system/fonts/Roboto-Bold.ttf", "/system/fonts/RobotoStatic-Bold.ttf", "/system/fonts/NotoSans-Bold.ttf"]),
        )
    }
    #[cfg(not(target_os = "android"))]
    {
        let regular = fc_match("sans-serif:style=Regular").or_else(|| fc_match("sans-serif"));
        let bold = fc_match("sans-serif:style=Bold").or_else(|| fc_match("sans-serif:weight=200"));
        (regular, bold)
    }
}

#[cfg(not(target_os = "android"))]
fn fc_match(pattern: &str) -> Option<String> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    let ok = (path.ends_with(".ttf") || path.ends_with(".otf")) && std::path::Path::new(path).exists();
    ok.then(|| path.to_string())
}

// ---------------------------------------------------------------------------------------------
// On-screen keyboard (Android)
// ---------------------------------------------------------------------------------------------

/// Show or hide the soft keyboard to match whether a text field has focus.
pub fn set_soft_keyboard(_visible: bool) {
    #[cfg(target_os = "android")]
    if let Some(app) = ANDROID_APP.get() {
        if _visible {
            app.show_soft_input(true);
        } else {
            app.hide_soft_input(true);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// System bars (Android draws the app edge to edge)
// ---------------------------------------------------------------------------------------------

/// Heights in physical pixels of what overlaps the app at the top (status bar) and bottom
/// (navigation bar, or the on-screen keyboard while it is open). Zero on the desktop.
/// `window_height_px` is the full window height.
pub fn system_bar_insets_px(window_height_px: f32) -> (f32, f32) {
    #[cfg(target_os = "android")]
    {
        static BARS: std::sync::OnceLock<(f32, f32)> = std::sync::OnceLock::new();
        let (status, nav) = *BARS.get_or_init(|| android::system_bar_heights().unwrap_or((0.0, 0.0)));
        // Android tells NativeActivity which part of the window is not covered (it shrinks
        // while the keyboard is open, with windowSoftInputMode=adjustResize).
        let (mut top, mut bottom) = (status, nav.max(android::ime_height_cached()));
        if let Some(app) = ANDROID_APP.get() {
            let r = app.content_rect();
            if r.bottom > r.top && r.bottom > 0 {
                top = top.max(r.top as f32);
                bottom = bottom.max(window_height_px - r.bottom as f32);
            }
        }
        (top, bottom.max(0.0))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = window_height_px;
        (0.0, 0.0)
    }
}

/// Current on-screen keyboard height in pixels (0 when hidden or on the desktop).
pub fn keyboard_height_px() -> f32 {
    #[cfg(target_os = "android")]
    {
        android::ime_height_cached()
    }
    #[cfg(not(target_os = "android"))]
    {
        0.0
    }
}

// ---------------------------------------------------------------------------------------------
// Saving and sharing
// ---------------------------------------------------------------------------------------------

/// Save bytes as a user-visible file. Desktop: a Save dialog (None = cancelled).
/// Android: straight into Downloads. Returns a human description of where it went.
pub async fn save_file(name: &str, mime: &str, bytes: Vec<u8>) -> Result<Option<String>> {
    #[cfg(target_os = "android")]
    {
        let name = name.to_string();
        let mime = mime.to_string();
        let r = tokio::task::spawn_blocking(move || android::save_to_downloads(&name, &mime, &bytes)).await??;
        Ok(Some(r))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = mime;
        let ext = name.rsplit('.').next().unwrap_or("").to_string();
        let Some(file) = rfd::AsyncFileDialog::new().set_file_name(name).add_filter(ext.to_uppercase(), &[ext.as_str()]).save_file().await else {
            return Ok(None);
        };
        std::fs::write(file.path(), bytes)?;
        Ok(Some(format!("Saved {}", file.path().display())))
    }
}

/// Share text (Android share sheet / desktop: open the mail client with it).
pub fn share_text(subject: &str, text: &str) -> Result<()> {
    #[cfg(target_os = "android")]
    {
        android::share_text(subject, text)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (subject, text);
        anyhow::bail!("use export::email on the desktop")
    }
}

#[cfg(target_os = "android")]
mod android {
    use anyhow::{Context, Result};
    use jni::JavaVM;
    use jni::objects::{JObject, JValue};

    fn with_env<R>(f: impl FnOnce(&mut jni::JNIEnv, &JObject) -> Result<R>) -> Result<R> {
        let ctx = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.context("JavaVM")?;
        let mut env = vm.attach_current_thread().context("attach JNI thread")?;
        // The Activity itself (window, insets); the Android context is only the Application.
        let raw = super::ANDROID_APP.get().map(|a| a.activity_as_ptr()).unwrap_or(ctx.context());
        let activity = unsafe { JObject::from_raw(raw.cast()) };
        let r = f(&mut env, &activity);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            anyhow::bail!("the Android system refused the request");
        }
        r
    }

    /// Status bar and navigation bar heights (px), from the system dimension resources.
    pub fn system_bar_heights() -> Result<(f32, f32)> {
        with_env(|env, activity| {
            let res = env.call_method(activity, "getResources", "()Landroid/content/res/Resources;", &[])?.l()?;
            let mut dim = |name: &str| -> Result<f32> {
                let n = env.new_string(name)?;
                let t = env.new_string("dimen")?;
                let p = env.new_string("android")?;
                let id = env.call_method(&res, "getIdentifier", "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I", &[JValue::Object(&n.into()), JValue::Object(&t.into()), JValue::Object(&p.into())])?.i()?;
                if id == 0 {
                    return Ok(0.0);
                }
                Ok(env.call_method(&res, "getDimensionPixelSize", "(I)I", &[JValue::Int(id)])?.i()? as f32)
            };
            Ok((dim("status_bar_height")?, dim("navigation_bar_height")?))
        })
    }

    /// Height of the on-screen keyboard (px), from WindowInsets (Android 11+); checked at most
    /// every 200 ms since it is a JNI round trip.
    pub fn ime_height_cached() -> f32 {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        static LAST: Mutex<Option<(Instant, f32)>> = Mutex::new(None);
        let mut g = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((t, h)) = *g
            && t.elapsed() < Duration::from_millis(200)
        {
            return h;
        }
        let h = match ime_height() {
            Ok(h) => h,
            Err(e) => {
                log::warn!("keyboard height: {e:#}");
                0.0
            }
        };

        *g = Some((Instant::now(), h));
        h
    }

    fn ime_height() -> Result<f32> {
        with_env(|env, activity| {
            let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
            if sdk < 30 {
                return Ok(0.0);
            }
            let window = env.call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?.l()?;
            let decor = env.call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?.l()?;
            let insets = env.call_method(&decor, "getRootWindowInsets", "()Landroid/view/WindowInsets;", &[])?.l()?;
            if insets.is_null() {
                return Ok(0.0);
            }
            let ime_type = env.call_static_method("android/view/WindowInsets$Type", "ime", "()I", &[])?.i()?;
            let ins = env.call_method(&insets, "getInsets", "(I)Landroid/graphics/Insets;", &[JValue::Int(ime_type)])?.l()?;
            Ok(env.get_field(&ins, "bottom", "I")?.i()? as f32)
        })
    }

    /// ACTION_SEND with the text, through the system chooser.
    pub fn share_text(subject: &str, text: &str) -> Result<()> {
        with_env(|env, activity| {
            let intent = env.new_object("android/content/Intent", "(Ljava/lang/String;)V", &[JValue::Object(&env.new_string("android.intent.action.SEND")?.into())])?;
            let mime = env.new_string("text/plain")?;
            env.call_method(&intent, "setType", "(Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&mime.into())])?;
            for (k, v) in [("android.intent.extra.SUBJECT", subject), ("android.intent.extra.TEXT", text)] {
                let k = env.new_string(k)?;
                let v = env.new_string(v)?;
                env.call_method(&intent, "putExtra", "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", &[JValue::Object(&k.into()), JValue::Object(&v.into())])?;
            }
            let title = env.new_string("Share article")?;
            let chooser = env.call_static_method(
                "android/content/Intent",
                "createChooser",
                "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
                &[JValue::Object(&intent), JValue::Object(&title.into())],
            )?;
            let chooser = chooser.l()?;
            // FLAG_ACTIVITY_NEW_TASK, since we may be called off the UI thread.
            env.call_method(&chooser, "addFlags", "(I)Landroid/content/Intent;", &[JValue::Int(0x1000_0000)])?;
            env.call_method(activity, "startActivity", "(Landroid/content/Intent;)V", &[JValue::Object(&chooser)])?;
            Ok(())
        })
    }

    /// Write a file into the shared Downloads collection (MediaStore, Android 10+); on older
    /// versions, into the app's own external files folder.
    pub fn save_to_downloads(name: &str, mime: &str, bytes: &[u8]) -> Result<String> {
        with_env(|env, activity| {
            let sdk = env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i()?;
            if sdk >= 29 {
                let resolver = env.call_method(activity, "getContentResolver", "()Landroid/content/ContentResolver;", &[])?.l()?;
                let values = env.new_object("android/content/ContentValues", "()V", &[])?;
                for (k, v) in [("_display_name", name), ("mime_type", mime), ("relative_path", "Download/Gateless Scholar")] {
                    let k = env.new_string(k)?;
                    let v = env.new_string(v)?;
                    env.call_method(&values, "put", "(Ljava/lang/String;Ljava/lang/String;)V", &[JValue::Object(&k.into()), JValue::Object(&v.into())])?;
                }
                let collection = env.get_static_field("android/provider/MediaStore$Downloads", "EXTERNAL_CONTENT_URI", "Landroid/net/Uri;")?.l()?;
                let uri = env.call_method(&resolver, "insert", "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;", &[JValue::Object(&collection), JValue::Object(&values)])?.l()?;
                if uri.is_null() {
                    anyhow::bail!("could not create the file in Downloads");
                }
                let stream = env.call_method(&resolver, "openOutputStream", "(Landroid/net/Uri;)Ljava/io/OutputStream;", &[JValue::Object(&uri)])?.l()?;
                let arr = env.byte_array_from_slice(bytes)?;
                env.call_method(&stream, "write", "([B)V", &[JValue::Object(&arr.into())])?;
                env.call_method(&stream, "close", "()V", &[])?;
                Ok(format!("Saved to Downloads/Gateless Scholar/{name}"))
            } else {
                let dir = env.call_method(activity, "getExternalFilesDir", "(Ljava/lang/String;)Ljava/io/File;", &[JValue::Object(&JObject::null())])?.l()?;
                let path = env.call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?.l()?;
                let path: String = env.get_string(&path.into())?.into();
                let full = std::path::Path::new(&path).join(name);
                std::fs::write(&full, bytes)?;
                Ok(format!("Saved to {}", full.display()))
            }
        })
    }
}
