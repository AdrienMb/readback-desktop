//! Profile and credential persistence.
//!
//! - Non-secret data (issued client ID, subject, email, scopes, expiry) lives in
//!   `chatgpt-profile.json` in the app data dir, written atomically with `0600` permissions.
//! - Secrets (access, refresh and ID tokens, plus a refresh rotation awaiting identity
//!   verification) live in one Keychain item per registration, so they are always replaced
//!   together.

#[cfg(test)]
use std::collections::HashMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{ErrorKind as IoErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::errors::{Error, Result};
use super::oauth::SHARING_SCOPE;
use super::secret::Secret;

pub const PROFILE_FILE: &str = "chatgpt-profile.json";
const MAX_FILE_BYTES: u64 = 1024 * 1024;

// ---------------------------------------------------------------------------------------------
// Non-secret profile
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    SignedOut,
    Connected,
    ReauthRequired,
}

/// The app registration for one ChatGPT account. Survives sign-out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registration {
    /// The issued `client_id` (never `dynamic_agent_client`).
    pub client_id: String,
    /// Verified `sub` of the ID token; `None` until the first sign-in completes.
    pub subject: Option<String>,
    /// Display label only; never used as identity.
    pub email: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionRecord {
    pub status: Status,
    pub scopes: Vec<String>,
    /// Access-token expiry, Unix milliseconds.
    pub expires_at: Option<i64>,
    /// Do not refresh before this time, Unix milliseconds.
    pub earliest_refresh_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub version: u32,
    pub registration: Option<Registration>,
    #[serde(default)]
    pub session: SessionRecord,
    #[serde(default)]
    pub welcome_shown: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            version: 1,
            registration: None,
            session: SessionRecord::default(),
            welcome_shown: false,
        }
    }
}

impl Profile {
    /// ChatGPT plan usage is usable only when connected with the direct-use scope granted.
    pub fn sharing(&self) -> bool {
        self.session.status == Status::Connected
            && self
                .session
                .scopes
                .iter()
                .any(|scope| scope == SHARING_SCOPE)
    }
}

// ---------------------------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Tokens {
    pub access: Secret,
    pub refresh: Option<Secret>,
    pub id: Option<Secret>,
}

/// Credentials received from a refresh, saved before their ID token is verified (the old
/// refresh token is already spent). Not usable until verification succeeds.
#[derive(Debug, Clone)]
pub struct PendingRotation {
    pub tokens: Tokens,
    pub scopes: Vec<String>,
    pub expires_at: i64,
    pub earliest_refresh_at: Option<i64>,
    /// When the response was received, Unix seconds; the ID token is checked as of this time.
    pub received_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Vault {
    pub current: Option<Tokens>,
    pub pending: Option<PendingRotation>,
}

impl Vault {
    /// The refresh token to revoke on sign-out: the newest one we hold.
    pub fn latest_refresh(&self) -> Option<&Secret> {
        self.pending
            .as_ref()
            .and_then(|pending| pending.tokens.refresh.as_ref())
            .or_else(|| {
                self.current
                    .as_ref()
                    .and_then(|tokens| tokens.refresh.as_ref())
            })
    }
}

// Wire format for the Keychain item. Private, zeroed on drop, never logged.
#[derive(Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    current: Option<TokensFile>,
    pending: Option<PendingFile>,
}

#[derive(Serialize, Deserialize)]
struct TokensFile {
    access: String,
    refresh: Option<String>,
    id: Option<String>,
}

impl Drop for TokensFile {
    fn drop(&mut self) {
        self.access.zeroize();
        self.refresh.zeroize();
        self.id.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
struct PendingFile {
    tokens: TokensFile,
    scopes: Vec<String>,
    expires_at: i64,
    earliest_refresh_at: Option<i64>,
    received_at: i64,
}

impl From<&Tokens> for TokensFile {
    fn from(tokens: &Tokens) -> Self {
        Self {
            access: tokens.access.expose().to_owned(),
            refresh: tokens.refresh.as_ref().map(|s| s.expose().to_owned()),
            id: tokens.id.as_ref().map(|s| s.expose().to_owned()),
        }
    }
}

impl From<&TokensFile> for Tokens {
    fn from(file: &TokensFile) -> Self {
        Self {
            access: Secret::new(file.access.as_str()),
            refresh: file.refresh.as_deref().map(Secret::new),
            id: file.id.as_deref().map(Secret::new),
        }
    }
}

fn encode_vault(vault: &Vault) -> Result<Zeroizing<Vec<u8>>> {
    let file = VaultFile {
        version: 1,
        current: vault.current.as_ref().map(TokensFile::from),
        pending: vault.pending.as_ref().map(|pending| PendingFile {
            tokens: TokensFile::from(&pending.tokens),
            scopes: pending.scopes.clone(),
            expires_at: pending.expires_at,
            earliest_refresh_at: pending.earliest_refresh_at,
            received_at: pending.received_at,
        }),
    };
    serde_json::to_vec(&file)
        .map(Zeroizing::new)
        .map_err(|_| vault_invalid())
}

fn decode_vault(bytes: &[u8]) -> Result<Vault> {
    let file: VaultFile = serde_json::from_slice(bytes).map_err(|_| vault_invalid())?;
    if file.version != 1 {
        return Err(vault_invalid());
    }
    Ok(Vault {
        current: file.current.as_ref().map(Tokens::from),
        pending: file.pending.as_ref().map(|pending| PendingRotation {
            tokens: Tokens::from(&pending.tokens),
            scopes: pending.scopes.clone(),
            expires_at: pending.expires_at,
            earliest_refresh_at: pending.earliest_refresh_at,
            received_at: pending.received_at,
        }),
    })
}

fn vault_invalid() -> Error {
    Error::storage(
        "credentials_invalid",
        "Readback's saved ChatGPT credentials couldn't be read. Sign in again.",
    )
}

/// Where secrets are kept. The Keychain in the app, memory in tests.
pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>>;
    fn set(&self, account: &str, value: &[u8]) -> Result<()>;
    fn delete(&self, account: &str) -> Result<()>;
}

#[cfg(target_os = "macos")]
pub use keychain::Keychain;

#[cfg(target_os = "macos")]
mod keychain {
    use std::sync::Arc;

    use keyring_core::api::CredentialStoreApi;
    use zeroize::Zeroizing;

    use super::{Error, Result, SecretStore};

    /// Keychain service name for Readback's ChatGPT credentials; the account is the client ID.
    const SERVICE: &str = "app.readback.desktop.chatgpt";

    pub struct Keychain {
        store: Arc<apple_native_keyring_store::keychain::Store>,
    }

    impl Keychain {
        pub fn new() -> Result<Self> {
            let store =
                apple_native_keyring_store::keychain::Store::new().map_err(keychain_error)?;
            Ok(Self { store })
        }

        fn entry(&self, account: &str) -> Result<keyring_core::Entry> {
            self.store
                .build(SERVICE, account, None)
                .map_err(keychain_error)
        }
    }

    impl SecretStore for Keychain {
        fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
            match self.entry(account)?.get_secret() {
                Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(error) => Err(keychain_error(error)),
            }
        }

        fn set(&self, account: &str, value: &[u8]) -> Result<()> {
            self.entry(account)?
                .set_secret(value)
                .map_err(keychain_error)
        }

        fn delete(&self, account: &str) -> Result<()> {
            match self.entry(account)?.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(error) => Err(keychain_error(error)),
            }
        }
    }

    // keyring errors can carry the stored bytes (BadEncoding), so they are never formatted.
    fn keychain_error(error: keyring_core::Error) -> Error {
        let code = match error {
            keyring_core::Error::NoStorageAccess(_) => "keychain_locked",
            keyring_core::Error::TooLong(..) => "keychain_too_long",
            _ => "keychain_error",
        };
        Error::storage(
            code,
            "Readback couldn't use the macOS Keychain. Unlock it, allow access, and try again.",
        )
    }
}

/// In-memory secret store for tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, Vec<u8>>>);

#[cfg(test)]
impl SecretStore for MemorySecrets {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(self
            .0
            .lock()
            .expect("lock")
            .get(account)
            .cloned()
            .map(Zeroizing::new))
    }

    fn set(&self, account: &str, value: &[u8]) -> Result<()> {
        self.0
            .lock()
            .expect("lock")
            .insert(account.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.0.lock().expect("lock").remove(account);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------------------------

pub struct Store {
    dir: PathBuf,
    secrets: Box<dyn SecretStore>,
}

impl Store {
    pub fn new(dir: PathBuf, secrets: Box<dyn SecretStore>) -> Self {
        Self { dir, secrets }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn load_profile(&self) -> Result<Profile> {
        let profile: Profile = read_json(&self.dir.join(PROFILE_FILE))?.unwrap_or_default();
        let valid = profile.version == 1
            && profile.registration.as_ref().is_none_or(|registration| {
                super::oauth::is_valid_client_id(&registration.client_id)
            });
        if !valid {
            return Err(Error::storage(
                "profile_invalid",
                "Readback's saved ChatGPT profile is damaged. Remove chatgpt-profile.json from \
                 the app data folder and sign in again.",
            ));
        }
        Ok(profile)
    }

    pub fn save_profile(&self, profile: &Profile) -> Result<()> {
        write_json_atomic(&self.dir.join(PROFILE_FILE), profile)
    }

    pub fn load_vault(&self, client_id: &str) -> Result<Option<Vault>> {
        match self.secrets.get(client_id)? {
            Some(bytes) => decode_vault(&bytes).map(Some),
            None => Ok(None),
        }
    }

    pub fn save_vault(&self, client_id: &str, vault: &Vault) -> Result<()> {
        self.secrets.set(client_id, &encode_vault(vault)?)
    }

    pub fn delete_vault(&self, client_id: &str) -> Result<()> {
        self.secrets.delete(client_id)
    }
}

fn storage_error() -> Error {
    Error::storage(
        "storage_error",
        "Readback couldn't save its ChatGPT settings. Check the app data folder's permissions.",
    )
}

fn ensure_private_dir(dir: &Path) -> Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|_| storage_error())?;
    let metadata = fs::symlink_metadata(dir).map_err(|_| storage_error())?;
    if !metadata.is_dir() {
        return Err(storage_error());
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|_| storage_error())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == IoErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(storage_error()),
    };
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(storage_error());
    }
    let bytes = fs::read(path).map_err(|_| storage_error())?;
    serde_json::from_slice(&bytes).map(Some).map_err(|_| {
        Error::storage(
            "profile_invalid",
            "Readback's saved ChatGPT settings are damaged. Remove them from the app data \
             folder and sign in again.",
        )
    })
}

/// Writes JSON to a `0600` temporary file, syncs it, then renames it over `path`.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let dir = path.parent().ok_or_else(storage_error)?;
    ensure_private_dir(dir)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(storage_error)?;
    let temporary = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| storage_error())?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|_| storage_error())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("readback-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn profile_round_trip_with_private_permissions() {
        let dir = temp_dir();
        let store = Store::new(dir.clone(), Box::new(MemorySecrets::default()));
        assert!(store.load_profile().unwrap().registration.is_none());

        let profile = Profile {
            registration: Some(Registration {
                client_id: "oaiapp_abc123".into(),
                subject: Some("user-1".into()),
                email: Some("me@example.com".into()),
            }),
            session: SessionRecord {
                status: Status::Connected,
                scopes: vec!["openid".into(), SHARING_SCOPE.into()],
                ..SessionRecord::default()
            },
            ..Profile::default()
        };
        store.save_profile(&profile).unwrap();

        let loaded = store.load_profile().unwrap();
        assert!(loaded.sharing());
        assert_eq!(loaded.registration.unwrap().client_id, "oaiapp_abc123");
        let mode = fs::metadata(dir.join(PROFILE_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn profile_never_stores_dynamic_client_id() {
        let dir = temp_dir();
        let store = Store::new(dir.clone(), Box::new(MemorySecrets::default()));
        let profile = Profile {
            registration: Some(Registration {
                client_id: "dynamic_agent_client".into(),
                subject: None,
                email: None,
            }),
            ..Profile::default()
        };
        store.save_profile(&profile).unwrap();
        assert_eq!(store.load_profile().unwrap_err().code, "profile_invalid");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn vault_round_trip_and_profile_holds_no_token() {
        let dir = temp_dir();
        let store = Store::new(dir.clone(), Box::new(MemorySecrets::default()));
        let vault = Vault {
            current: Some(Tokens {
                access: Secret::new("access-AAA"),
                refresh: Some(Secret::new("refresh-BBB")),
                id: Some(Secret::new("id-CCC")),
            }),
            pending: None,
        };
        store.save_vault("oaiapp_x", &vault).unwrap();
        let loaded = store.load_vault("oaiapp_x").unwrap().unwrap();
        assert_eq!(
            loaded.current.as_ref().unwrap().access.expose(),
            "access-AAA"
        );
        assert_eq!(loaded.latest_refresh().unwrap().expose(), "refresh-BBB");
        assert!(!format!("{loaded:?}").contains("AAA"));

        store.save_profile(&Profile::default()).unwrap();
        let on_disk = fs::read_to_string(dir.join(PROFILE_FILE)).unwrap();
        assert!(!on_disk.contains("access-AAA") && !on_disk.contains("refresh-BBB"));

        store.delete_vault("oaiapp_x").unwrap();
        assert!(store.load_vault("oaiapp_x").unwrap().is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}
