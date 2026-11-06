//! Sign in with ChatGPT runtime. Everything that touches a credential lives here; the webview
//! only ever receives [`SessionView`], model names, text deltas and [`Error`]s.

pub mod commands;
pub mod errors;
pub mod host_id;
pub mod id_token;
pub mod inference;
pub mod oauth;
pub mod refresh;
pub mod secret;
pub mod store;

use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Client;
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;

use errors::{Error, ErrorKind, Result};
use id_token::IdTokenVerifier;
use inference::Model;
use oauth::{AuthorizeRequest, CodeExchange, Discovery, ISSUER, Loopback};
use refresh::Decision;
use secret::Secret;
use store::{PendingRotation, Profile, Registration, SessionRecord, Status, Store, Tokens, Vault};

pub const USAGE_URL: &str = "https://chatgpt.com/settings/usage";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_PROMPT_BYTES: usize = 100 * 1024;

/// Display data only. Never contains a credential.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub status: Status,
    pub email: Option<String>,
    /// True only when connected and `chatgpt.tokens.use.direct` was granted.
    pub sharing: bool,
    /// The one-time "You're using your ChatGPT plan" welcome.
    pub show_welcome: bool,
}

impl From<&Profile> for SessionView {
    fn from(profile: &Profile) -> Self {
        let status = match (profile.registration.as_ref(), profile.session.status) {
            (None, _) => Status::SignedOut,
            (Some(_), status) => status,
        };
        let sharing = profile.sharing();
        Self {
            status,
            email: profile.registration.as_ref().and_then(|r| r.email.clone()),
            sharing,
            show_welcome: sharing && !profile.welcome_shown,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignOutView {
    pub session: SessionView,
    /// False when local credentials were removed but remote revocation couldn't be confirmed.
    pub revocation_confirmed: bool,
}

pub struct ChatGpt {
    app: AppHandle,
    http: Client,
    store: Store,
    verifier: IdTokenVerifier,
    discovery: Mutex<Option<Arc<Discovery>>>,
    /// Serializes sign-in, sign-out and refresh: the refresh token rotates.
    auth: Mutex<()>,
    sign_in_cancel: StdMutex<Option<CancellationToken>>,
    prompt_cancel: StdMutex<Option<CancellationToken>>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl ChatGpt {
    pub fn new(app: AppHandle, store: Store) -> Result<Self> {
        let http = Client::builder()
            .user_agent(concat!("Readback/", env!("CARGO_PKG_VERSION")))
            // A redirect could forward the bearer token or the prompt to another origin.
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| Error::network())?;
        Ok(Self {
            app,
            http,
            store,
            verifier: IdTokenVerifier::default(),
            discovery: Mutex::new(None),
            auth: Mutex::new(()),
            sign_in_cancel: StdMutex::new(None),
            prompt_cancel: StdMutex::new(None),
        })
    }

    /// Reads the profile file only (no Keychain, no network, no lock): safe at any time.
    pub fn session(&self) -> Result<SessionView> {
        Ok(SessionView::from(&self.store.load_profile()?))
    }

    async fn discovery(&self) -> Result<Arc<Discovery>> {
        let mut cached = self.discovery.lock().await;
        if let Some(discovery) = cached.as_ref() {
            return Ok(discovery.clone());
        }
        let discovery = Arc::new(oauth::discover(&self.http).await?);
        *cached = Some(discovery.clone());
        Ok(discovery)
    }

    // -----------------------------------------------------------------------------------------
    // Sign-in
    // -----------------------------------------------------------------------------------------

    pub async fn sign_in(&self, reconsent: bool) -> Result<SessionView> {
        self.cancel_prompt();
        let cancel = CancellationToken::new();
        if let Some(previous) = self
            .sign_in_cancel
            .lock()
            .expect("lock")
            .replace(cancel.clone())
        {
            previous.cancel();
        }
        let result = {
            let _auth = self.auth.lock().await;
            self.sign_in_locked(reconsent, &cancel).await
        };
        self.sign_in_cancel.lock().expect("lock").take();
        result?;
        self.session()
    }

    pub fn cancel_sign_in(&self) {
        if let Some(cancel) = self.sign_in_cancel.lock().expect("lock").take() {
            cancel.cancel();
        }
    }

    async fn sign_in_locked(&self, reconsent: bool, cancel: &CancellationToken) -> Result<()> {
        let host_id = host_id::load_or_create(self.store.dir())?;
        let mut profile = self.store.load_profile()?;
        let discovery = self.discovery().await?;
        let saved_client_id = profile.registration.as_ref().map(|r| r.client_id.clone());
        let login_hint = profile.registration.as_ref().and_then(|r| r.email.clone());

        let state = oauth::random_token();
        let nonce = oauth::random_token();
        let verifier = Secret::new(oauth::random_token());
        let code_challenge = oauth::pkce_challenge(verifier.expose());

        // Listen before opening the browser.
        let listener = Loopback::bind().await?;
        let redirect_uri = listener.redirect_uri();
        let url = oauth::authorize_url(
            &discovery.authorization_endpoint,
            &AuthorizeRequest {
                issued_client_id: saved_client_id.as_deref(),
                host_id: &host_id,
                redirect_uri: &redirect_uri,
                state: &state,
                nonce: &nonce,
                code_challenge: &code_challenge,
                login_hint: login_hint.as_deref(),
                reconsent,
            },
        );
        match &saved_client_id {
            Some(client_id) => {
                tracing::info!(client_id = %client_id, "ChatGPT sign-in: reusing issued client ID");
            }
            None => tracing::info!("ChatGPT sign-in: first registration (dynamic_agent_client)"),
        }
        self.open_auth_url(&url)?;

        let callback = listener
            .wait(&state, saved_client_id.as_deref(), cancel, SIGN_IN_TIMEOUT)
            .await?;

        // Save the issued client ID before the one-time code exchange, so that a failed or
        // expired code never leads to a second registration.
        if saved_client_id.is_none() {
            profile.registration = Some(Registration {
                client_id: callback.client_id.clone(),
                subject: None,
                email: None,
            });
            self.store.save_profile(&profile)?;
            tracing::info!(client_id = %callback.client_id, "ChatGPT registration saved");
        }

        let grant = oauth::exchange_code(
            &self.http,
            &discovery,
            &CodeExchange {
                client_id: &callback.client_id,
                code: &callback.code,
                verifier: &verifier,
                redirect_uri: &redirect_uri,
            },
            now_ms(),
        )
        .await?;
        let id_token = grant.id_token.clone().ok_or_else(Error::invalid_id_token)?;
        let identity = self
            .verifier
            .verify(
                &self.http,
                &discovery.jwks_uri,
                &id_token,
                &callback.client_id,
                Some(&nonce),
                now_ms() / 1000,
            )
            .await?;

        let registration = profile
            .registration
            .as_mut()
            .expect("registration saved above");
        if registration
            .subject
            .as_ref()
            .is_some_and(|saved| *saved != identity.subject)
        {
            return Err(Error::new(
                ErrorKind::SignIn,
                "account_mismatch",
                "This sign-in returned a different ChatGPT account than the one connected to \
                 Readback. Sign in with the original account.",
            ));
        }
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }

        // Secrets first, then the profile that marks them usable.
        self.store.save_vault(
            &callback.client_id,
            &Vault {
                current: Some(Tokens {
                    access: grant.access,
                    refresh: grant.refresh,
                    id: Some(id_token),
                }),
                pending: None,
            },
        )?;
        registration.subject = Some(identity.subject);
        registration.email = identity.email.or(registration.email.take());
        profile.session = SessionRecord {
            status: Status::Connected,
            scopes: grant.scopes,
            expires_at: Some(grant.expires_at),
            earliest_refresh_at: grant.earliest_refresh_at,
        };
        self.store.save_profile(&profile)?;
        tracing::info!(
            client_id = %callback.client_id,
            sharing = profile.sharing(),
            "ChatGPT sign-in complete"
        );
        Ok(())
    }

    fn open_auth_url(&self, url: &Url) -> Result<()> {
        let issuer = Url::parse(ISSUER).expect("valid issuer").origin();
        if url.origin() != issuer {
            return Err(Error::new(
                ErrorKind::SignIn,
                "unexpected_destination",
                "Unexpected sign-in address.",
            ));
        }
        // The URL holds no token (only state, nonce, PKCE challenge and login_hint).
        self.app.opener().open_url(url.as_str(), None::<&str>).map_err(|_| {
            Error::new(
                ErrorKind::SignIn,
                "browser_unavailable",
                "Readback couldn't open your browser. Check your default browser and try again.",
            )
        })
    }

    pub fn open_usage_settings(&self) -> Result<()> {
        self.app
            .opener()
            .open_url(USAGE_URL, None::<&str>)
            .map_err(|_| {
                Error::new(
                    ErrorKind::Other,
                    "browser_unavailable",
                    "Readback couldn't open your browser. Visit chatgpt.com/settings/usage.",
                )
            })
    }

    pub fn dismiss_welcome(&self) -> Result<SessionView> {
        let mut profile = self.store.load_profile()?;
        profile.welcome_shown = true;
        self.store.save_profile(&profile)?;
        Ok(SessionView::from(&profile))
    }

    // -----------------------------------------------------------------------------------------
    // Sign-out
    // -----------------------------------------------------------------------------------------

    /// Revokes the refresh token, then clears all tokens even if revocation failed. Keeps the
    /// registration (issued client ID, subject, email) and the host ID.
    pub async fn sign_out(&self) -> Result<SignOutView> {
        self.cancel_prompt();
        self.cancel_sign_in();
        let _auth = self.auth.lock().await;
        let mut profile = self.store.load_profile()?;
        let Some(client_id) = profile.registration.as_ref().map(|r| r.client_id.clone()) else {
            return Ok(SignOutView {
                session: SessionView::from(&profile),
                revocation_confirmed: true,
            });
        };
        let vault = self.store.load_vault(&client_id).ok().flatten();
        let revocation_confirmed = match vault.as_ref().and_then(Vault::latest_refresh) {
            None => true,
            Some(refresh) => match self.discovery().await {
                Ok(discovery) => match &discovery.revocation_endpoint {
                    Some(endpoint) => {
                        oauth::revoke(&self.http, endpoint, &client_id, refresh).await
                    }
                    None => false,
                },
                Err(_) => false,
            },
        };
        self.store.delete_vault(&client_id)?;
        profile.session = SessionRecord::default();
        self.store.save_profile(&profile)?;
        tracing::info!(client_id = %client_id, revocation_confirmed, "ChatGPT sign-out (registration kept)");
        Ok(SignOutView {
            session: SessionView::from(&profile),
            revocation_confirmed,
        })
    }

    // -----------------------------------------------------------------------------------------
    // Credentials for a request (with refresh)
    // -----------------------------------------------------------------------------------------

    async fn access_token(&self) -> Result<Secret> {
        let _auth = self.auth.lock().await;
        let mut profile = self.store.load_profile()?;
        let Some(client_id) = profile.registration.as_ref().map(|r| r.client_id.clone()) else {
            return Err(Error::sign_in_required());
        };
        match profile.session.status {
            Status::Connected => {}
            Status::ReauthRequired => return Err(Error::session_expired()),
            Status::SignedOut => return Err(Error::sign_in_required()),
        }
        if !profile.sharing() {
            return Err(Error::sharing_disabled());
        }
        let Some(mut vault) = self.store.load_vault(&client_id)? else {
            self.invalidate(&mut profile, &client_id)?;
            return Err(Error::session_expired());
        };

        if vault.pending.is_some() {
            return self.finish_rotation(&mut profile, &client_id, vault).await;
        }
        let current = vault.current.clone().ok_or_else(Error::session_expired)?;
        let now = now_ms();
        match refresh::decide(
            profile.session.expires_at,
            profile.session.earliest_refresh_at,
            now,
        ) {
            Decision::Fresh => return Ok(current.access),
            Decision::NotReady => {
                return Err(Error::new(
                    ErrorKind::Temporary,
                    "refresh_not_ready",
                    "Your ChatGPT connection can't be renewed yet. Try again in a moment.",
                ));
            }
            Decision::Refresh => {}
        }

        let refresh_token = current.refresh.clone().ok_or_else(Error::session_expired)?;
        let discovery = self.discovery().await?;
        let grant = match refresh::refresh_tokens(
            &self.http,
            &discovery,
            &client_id,
            &refresh_token,
            &profile.session.scopes,
            now,
        )
        .await
        {
            Ok(grant) => grant,
            Err(error) if error.is_terminal_refresh() || error.code == "invalid_token_response" => {
                self.invalidate(&mut profile, &client_id)?;
                return Err(Error::session_expired());
            }
            // Network or temporary failure: keep the credentials.
            Err(error) => return Err(error),
        };
        tracing::info!(client_id = %client_id, "ChatGPT tokens refreshed");

        match grant.id_token {
            Some(id_token) => {
                // The old refresh token is spent: save the rotation before verifying it.
                vault.pending = Some(PendingRotation {
                    tokens: Tokens {
                        access: grant.access,
                        refresh: grant.refresh,
                        id: Some(id_token),
                    },
                    scopes: grant.scopes,
                    expires_at: grant.expires_at,
                    earliest_refresh_at: grant.earliest_refresh_at,
                    received_at: now / 1000,
                });
                self.store.save_vault(&client_id, &vault)?;
                self.finish_rotation(&mut profile, &client_id, vault).await
            }
            None => {
                let access = grant.access.clone();
                vault.current = Some(Tokens {
                    access: grant.access,
                    refresh: grant.refresh,
                    id: current.id,
                });
                self.store.save_vault(&client_id, &vault)?;
                self.apply_session(
                    &mut profile,
                    grant.scopes,
                    grant.expires_at,
                    grant.earliest_refresh_at,
                )?;
                Ok(access)
            }
        }
    }

    /// Verifies a saved rotation's ID token (as of its receipt time) and promotes it.
    async fn finish_rotation(
        &self,
        profile: &mut Profile,
        client_id: &str,
        mut vault: Vault,
    ) -> Result<Secret> {
        let pending = vault.pending.take().expect("pending rotation");
        let id_token = pending
            .tokens
            .id
            .clone()
            .ok_or_else(Error::invalid_id_token)?;
        let discovery = self.discovery().await?;
        let verified = self
            .verifier
            .verify(
                &self.http,
                &discovery.jwks_uri,
                &id_token,
                client_id,
                None,
                pending.received_at,
            )
            .await;
        let identity = match verified {
            Ok(identity) => identity,
            // Keys unavailable: keep the pending rotation and try again later.
            Err(error) if error.code == "identity_verification_unavailable" => return Err(error),
            Err(_) => {
                self.invalidate(profile, client_id)?;
                return Err(Error::session_expired());
            }
        };
        let registration = profile.registration.as_mut().expect("registration");
        if registration
            .subject
            .as_ref()
            .is_some_and(|subject| *subject != identity.subject)
        {
            self.invalidate(profile, client_id)?;
            return Err(Error::session_expired());
        }
        if identity.email.is_some() {
            registration.email = identity.email;
        }
        let access = pending.tokens.access.clone();
        vault.current = Some(pending.tokens);
        self.store.save_vault(client_id, &vault)?;
        self.apply_session(
            profile,
            pending.scopes,
            pending.expires_at,
            pending.earliest_refresh_at,
        )?;
        Ok(access)
    }

    fn apply_session(
        &self,
        profile: &mut Profile,
        scopes: Vec<String>,
        expires_at: i64,
        earliest_refresh_at: Option<i64>,
    ) -> Result<()> {
        profile.session = SessionRecord {
            status: Status::Connected,
            scopes,
            expires_at: Some(expires_at),
            earliest_refresh_at,
        };
        self.store.save_profile(profile)
    }

    /// The token set is unusable: clear it, keep the registration, ask for sign-in.
    fn invalidate(&self, profile: &mut Profile, client_id: &str) -> Result<()> {
        self.store.delete_vault(client_id)?;
        profile.session = SessionRecord {
            status: Status::ReauthRequired,
            ..SessionRecord::default()
        };
        self.store.save_profile(profile)
    }

    // -----------------------------------------------------------------------------------------
    // Inference
    // -----------------------------------------------------------------------------------------

    pub async fn list_models(&self) -> Result<Vec<Model>> {
        let access = self.access_token().await?;
        inference::list_models(&self.http, &access).await
    }

    pub async fn send_prompt(
        &self,
        model: &str,
        prompt: &str,
        on_delta: impl FnMut(&str),
    ) -> Result<()> {
        let model = model.trim();
        if model.is_empty() || model.len() > 200 {
            return Err(Error::new(
                ErrorKind::Request,
                "invalid_model",
                "Choose a model first.",
            ));
        }
        if prompt.trim().is_empty() {
            return Err(Error::new(
                ErrorKind::Request,
                "empty_prompt",
                "Type a prompt first.",
            ));
        }
        if prompt.len() > MAX_PROMPT_BYTES {
            return Err(Error::new(
                ErrorKind::Request,
                "prompt_too_long",
                "This prompt is too long.",
            ));
        }
        let cancel = CancellationToken::new();
        if let Some(previous) = self
            .prompt_cancel
            .lock()
            .expect("lock")
            .replace(cancel.clone())
        {
            previous.cancel();
        }
        // Refresh isn't cancelled midway: abandoning it could lose the rotated refresh token.
        // (A finished request's token may stay in the slot; cancelling it later is a no-op.)
        let access = self.access_token().await?;
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        inference::stream_response(&self.http, &access, model, prompt, &cancel, on_delta).await
    }

    pub fn cancel_prompt(&self) {
        if let Some(cancel) = self.prompt_cancel.lock().expect("lock").take() {
            cancel.cancel();
        }
    }
}
