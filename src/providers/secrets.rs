//! Secrets in the system keyring (Secret Service on Linux); on Android, a file in the app's
//! private storage (see `platform::secret_file`).
//! TODO(windows): the keyring crate's Windows credential store backend needs enabling.
//!
//! The keyring calls block (D-Bus round trips) and the Secret Service backend drives its own
//! async runtime internally, which must not run on a thread that is already inside a tokio
//! runtime. Each call therefore runs on a short-lived plain thread, so these functions are
//! safe to call from the UI thread and from async tasks alike.

#[cfg(not(target_os = "android"))]
use anyhow::Context;

#[cfg(target_os = "android")]
pub fn get_api_key(id: &str) -> anyhow::Result<Option<String>> {
    crate::platform::secret_file::get(id)
}
#[cfg(target_os = "android")]
pub fn set_api_key(id: &str, key: &str) -> anyhow::Result<()> {
    crate::platform::secret_file::set(id, key.trim())
}
#[cfg(target_os = "android")]
pub fn delete_api_key(id: &str) -> anyhow::Result<()> {
    crate::platform::secret_file::delete(id)
}

pub const SERVICE: &str = "gateless-scholar";

#[cfg(not(target_os = "android"))]
fn on_plain_thread<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
    std::thread::spawn(f).join().map_err(|_| anyhow::anyhow!("keyring access panicked"))?
}

/// The API key saved for a provider id (or any named secret, e.g. "core-api-key").
#[cfg(not(target_os = "android"))]
pub fn get_api_key(id: &str) -> anyhow::Result<Option<String>> {
    let id = id.to_string();
    on_plain_thread(move || {
        let entry = keyring::Entry::new(SERVICE, &id).context("opening the system keyring")?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::Error::new(e).context("reading from the system keyring")),
        }
    })
}

#[cfg(not(target_os = "android"))]
pub fn set_api_key(id: &str, key: &str) -> anyhow::Result<()> {
    let (id, key) = (id.to_string(), key.trim().to_string());
    on_plain_thread(move || {
        let entry = keyring::Entry::new(SERVICE, &id).context("opening the system keyring")?;
        entry.set_password(&key).context("saving to the system keyring")
    })
}

#[cfg(not(target_os = "android"))]
pub fn delete_api_key(id: &str) -> anyhow::Result<()> {
    let id = id.to_string();
    on_plain_thread(move || {
        let entry = keyring::Entry::new(SERVICE, &id).context("opening the system keyring")?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow::Error::new(e).context("deleting from the system keyring")),
        }
    })
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    /// Touches the real keyring: `cargo test providers::secrets -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn live_roundtrip_inside_runtime() {
        let id = "gateless-test-secret";
        delete_api_key(id).unwrap();
        assert_eq!(get_api_key(id).unwrap(), None);
        set_api_key(id, "abc123").unwrap();
        assert_eq!(get_api_key(id).unwrap().as_deref(), Some("abc123"));
        delete_api_key(id).unwrap();
        assert_eq!(get_api_key(id).unwrap(), None);
    }
}
