//! OAuth for Sign in with ChatGPT: discovery, PKCE, state and nonce, the authorize URL, the
//! one-shot loopback listener, the code exchange, token parsing and revocation.
//!
//! Written from https://developers.openai.com/siwc/token-sharing-open-source (Registration and
//! sign-in, Accounts and sessions, Token reference).

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use reqwest::Client;
use reqwest::header::ACCEPT;
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::errors::{Error, ErrorKind, Result, api_error, safe_identifier};
use super::secret::Secret;

pub const ISSUER: &str = "https://auth.openai.com";
pub const RESOURCE: &str = "https://api.openai.com/v1";
pub const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
pub const SHARING_SCOPE: &str = "chatgpt.tokens.use.direct";
pub const CALLBACK_PATH: &str = "/auth/callback";
pub const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
pub const APP_NAME: &str = "Readback";

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Discovery {
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub revocation_endpoint: Option<Url>,
    pub jwks_uri: Url,
}

pub async fn discover(http: &Client) -> Result<Discovery> {
    let response = http
        .get(format!("{ISSUER}/.well-known/openid-configuration"))
        .header(ACCEPT, "application/json")
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|_| Error::network())?;
    if !response.status().is_success() {
        return Err(discovery_failed());
    }
    let body: Value = response.json().await.map_err(|_| discovery_failed())?;
    parse_discovery(&body)
}

fn discovery_failed() -> Error {
    Error::new(
        ErrorKind::Temporary,
        "discovery_failed",
        "ChatGPT's sign-in configuration couldn't be verified. Try again shortly.",
    )
}

/// Every endpoint must be on the issuer's origin.
pub fn parse_discovery(body: &Value) -> Result<Discovery> {
    if body.get("issuer").and_then(Value::as_str) != Some(ISSUER) {
        return Err(discovery_failed());
    }
    let issuer = Url::parse(ISSUER).expect("valid issuer").origin();
    let endpoint = |key: &str| -> Result<Option<Url>> {
        match body.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => match Url::parse(value) {
                Ok(url) if url.origin() == issuer => Ok(Some(url)),
                _ => Err(discovery_failed()),
            },
            Some(_) => Err(discovery_failed()),
        }
    };
    Ok(Discovery {
        authorization_endpoint: endpoint("authorization_endpoint")?.ok_or_else(discovery_failed)?,
        token_endpoint: endpoint("token_endpoint")?.ok_or_else(discovery_failed)?,
        revocation_endpoint: endpoint("revocation_endpoint")?,
        jwks_uri: endpoint("jwks_uri")?.ok_or_else(discovery_failed)?,
    })
}

// ---------------------------------------------------------------------------------------------
// PKCE, state, nonce, authorize URL
// ---------------------------------------------------------------------------------------------

/// 32 random bytes, base64url without padding. Used for state, nonce and the PKCE verifier.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// `code_challenge` for `S256`: base64url(SHA-256(verifier)) without padding.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub struct AuthorizeRequest<'a> {
    /// The issued client ID for a returning registration; `None` registers a new one.
    pub issued_client_id: Option<&'a str>,
    pub host_id: &'a str,
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub nonce: &'a str,
    pub code_challenge: &'a str,
    /// Saved email of the registration. We never send `id_token_hint`: the browser is opened
    /// with `/usr/bin/open`, which would expose the token in process arguments.
    pub login_hint: Option<&'a str>,
    /// `prompt=consent`, only after the user explicitly asks to enable plan usage.
    pub reconsent: bool,
}

pub fn authorize_url(endpoint: &Url, request: &AuthorizeRequest<'_>) -> Url {
    let mut url = endpoint.clone();
    {
        let mut query = url.query_pairs_mut();
        query.clear();
        query.append_pair(
            "client_id",
            request.issued_client_id.unwrap_or(DYNAMIC_CLIENT_ID),
        );
        query.append_pair("response_type", "code");
        query.append_pair("redirect_uri", request.redirect_uri);
        query.append_pair("scope", SCOPES);
        query.append_pair("resource", RESOURCE);
        query.append_pair("state", request.state);
        query.append_pair("nonce", request.nonce);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("code_challenge", request.code_challenge);
        query.append_pair("ext_agent_host_id", request.host_id);
        if request.issued_client_id.is_none() {
            query.append_pair("agent_name_hint", APP_NAME);
        }
        if let Some(hint) = request.login_hint {
            query.append_pair("login_hint", hint);
        }
        if request.reconsent {
            query.append_pair("prompt", "consent");
        }
    }
    url
}

pub fn is_valid_client_id(id: &str) -> bool {
    id != DYNAMIC_CLIENT_ID
        && (1..=200).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ---------------------------------------------------------------------------------------------
// Callback
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct Callback {
    pub code: Secret,
    pub client_id: String,
}

#[derive(Debug)]
pub enum CallbackResult {
    /// Missing, repeated or wrong `state`: not our redirect. The pending attempt continues.
    InvalidState,
    /// Our redirect: sign-in finishes with this result.
    Done(Result<Callback>),
}

pub fn parse_callback(
    query: &str,
    expected_state: &str,
    saved_client_id: Option<&str>,
) -> CallbackResult {
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let values = |key: &str| -> Vec<&str> {
        pairs
            .iter()
            .filter(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
            .collect()
    };

    // Validate state first, in constant time, before looking at anything else.
    let states = values("state");
    if states.len() != 1 || !bool::from(states[0].as_bytes().ct_eq(expected_state.as_bytes())) {
        return CallbackResult::InvalidState;
    }

    if let Some(error) = values("error").first() {
        let code = safe_identifier(error).unwrap_or("authorization_error");
        return CallbackResult::Done(Err(if code == "access_denied" {
            api_error(
                Some(&serde_json::json!({ "error": "access_denied" })),
                None,
                None,
            )
        } else {
            Error::new(
                ErrorKind::SignIn,
                code,
                "ChatGPT didn't complete sign-in. Try again.",
            )
        }));
    }

    let incomplete = || {
        Error::new(
            ErrorKind::SignIn,
            "registration_incomplete",
            "ChatGPT didn't complete Readback's app registration. Try signing in again.",
        )
    };
    let codes = values("code");
    let client_ids = values("client_id");
    if codes.len() != 1 || codes[0].is_empty() || client_ids.len() > 1 {
        return CallbackResult::Done(Err(incomplete()));
    }
    let returned = client_ids.first().copied();
    if let (Some(saved), Some(returned)) = (saved_client_id, returned)
        && saved != returned
    {
        return CallbackResult::Done(Err(Error::new(
            ErrorKind::SignIn,
            "client_id_mismatch",
            "ChatGPT returned a different app registration than the one saved on this Mac, so \
             sign-in was stopped.",
        )));
    }
    // On reauthorization the callback may omit client_id: keep the saved one.
    match returned.or(saved_client_id) {
        Some(client_id) if is_valid_client_id(client_id) => CallbackResult::Done(Ok(Callback {
            code: Secret::new(codes[0]),
            client_id: client_id.to_owned(),
        })),
        _ => CallbackResult::Done(Err(incomplete())),
    }
}

// ---------------------------------------------------------------------------------------------
// Loopback listener
// ---------------------------------------------------------------------------------------------

const MAX_HEAD_BYTES: usize = 8 * 1024;
const PAGE_SCRIPT: &str = "history.replaceState(null, '', '/auth/complete');";

/// A one-shot HTTP listener on `127.0.0.1:<free port>`. Bind it before opening the browser.
pub struct Loopback {
    listener: TcpListener,
    port: u16,
}

impl Loopback {
    pub async fn bind() -> Result<Self> {
        let unavailable = || {
            Error::new(
                ErrorKind::SignIn,
                "callback_port_unavailable",
                "Readback couldn't start its local sign-in listener. Try again.",
            )
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| unavailable())?;
        let port = listener.local_addr().map_err(|_| unavailable())?.port();
        Ok(Self { listener, port })
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    /// Serves until our redirect arrives, the timeout passes or `cancel` fires, then closes.
    pub async fn wait(
        self,
        state: &str,
        saved_client_id: Option<&str>,
        cancel: &CancellationToken,
        timeout: Duration,
    ) -> Result<Callback> {
        let (sender, mut receiver) = mpsc::channel::<Result<Callback>>(1);
        let state: Arc<str> = state.into();
        let saved: Option<Arc<str>> = saved_client_id.map(Into::into);
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(Error::new(ErrorKind::Cancelled, "cancelled", "Sign-in was cancelled."));
                }
                _ = &mut deadline => {
                    return Err(Error::new(
                        ErrorKind::SignIn,
                        "sign_in_timeout",
                        "Sign-in timed out. Start again when you're ready.",
                    ));
                }
                Some(result) = receiver.recv() => return result,
                accepted = self.listener.accept() => {
                    if let Ok((stream, _)) = accepted {
                        // One task per connection: a browser preconnect that never sends a
                        // request must not block the real redirect.
                        tokio::spawn(serve(
                            stream, self.port, state.clone(), saved.clone(), sender.clone(),
                        ));
                    }
                }
            }
        }
    }
}

async fn serve(
    mut stream: TcpStream,
    port: u16,
    state: Arc<str>,
    saved_client_id: Option<Arc<str>>,
    sender: mpsc::Sender<Result<Callback>>,
) {
    let Ok(Some(head)) =
        tokio::time::timeout(Duration::from_secs(10), read_head(&mut stream)).await
    else {
        return;
    };
    let (response, outcome) = handle_request(&head, port, &state, saved_client_id.as_deref());
    let _ = stream.write_all(&response).await;
    let _ = stream.shutdown().await;
    if let Some(outcome) = outcome {
        let _ = sender.try_send(outcome);
    }
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            buffer.truncate(end);
            return String::from_utf8(buffer).ok();
        }
        if buffer.len() > MAX_HEAD_BYTES {
            return None;
        }
    }
}

/// Handles one HTTP request head. Returns the raw response, and the sign-in result when the
/// request was our redirect (correct method, host, path and state).
pub fn handle_request(
    head: &str,
    port: u16,
    state: &str,
    saved_client_id: Option<&str>,
) -> (Vec<u8>, Option<Result<Callback>>) {
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let (method, target, version) = (
        request_line.next(),
        request_line.next(),
        request_line.next(),
    );
    let host = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("host")
            .then(|| value.trim())
    });
    let expected_host = format!("127.0.0.1:{port}");
    let target = target.filter(|t| t.starts_with('/'));
    let url = target.and_then(|t| Url::parse(&format!("http://{expected_host}{t}")).ok());
    let ours = method == Some("GET")
        && version.is_some_and(|v| v.starts_with("HTTP/1."))
        && host == Some(expected_host.as_str());
    let Some(url) = url.filter(|url| ours && url.path() == CALLBACK_PATH) else {
        return (http_response(404, "text/plain", "Not found"), None);
    };
    match parse_callback(url.query().unwrap_or_default(), state, saved_client_id) {
        CallbackResult::InvalidState => (
            http_response(
                400,
                "text/plain",
                "Invalid sign-in state. Return to the browser tab that started sign-in.",
            ),
            None,
        ),
        CallbackResult::Done(result) => {
            let page = format!(
                "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Return to \
                 Readback</title><style>body{{font:17px -apple-system,system-ui,sans-serif;\
                 max-width:32rem;margin:18vh auto;padding:24px;color:#202123}}h1{{font-size:\
                 26px}}</style><h1>Return to Readback</h1><p>Readback is finishing your \
                 ChatGPT sign-in. You can close this tab.</p><script>{PAGE_SCRIPT}</script>\
                 </html>"
            );
            (
                http_response(200, "text/html; charset=utf-8", &page),
                Some(result),
            )
        }
    }
}

fn http_response(status: u16, content_type: &str, body: &str) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let script_hash = STANDARD.encode(Sha256::digest(PAGE_SCRIPT.as_bytes()));
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: \
         nosniff\r\nContent-Security-Policy: default-src 'none'; script-src 'sha256-{script_hash}'; \
         style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'\r\nConnection: \
         close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

// ---------------------------------------------------------------------------------------------
// Token endpoint
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct TokenGrant {
    pub access: Secret,
    pub refresh: Option<Secret>,
    pub id_token: Option<Secret>,
    pub scopes: Vec<String>,
    /// Unix milliseconds.
    pub expires_at: i64,
    /// Unix milliseconds.
    pub earliest_refresh_at: Option<i64>,
}

fn incomplete_credentials() -> Error {
    Error::new(
        ErrorKind::Reauth,
        "invalid_token_response",
        "ChatGPT returned incomplete credentials. Sign in again.",
    )
}

/// Validates a token-endpoint response. `previous_scopes` is used when a refresh omits `scope`
/// (unchanged grant). A refresh token is required for refreshes and whenever `offline_access`
/// was granted.
pub fn parse_token_response(
    data: &Value,
    previous_scopes: Option<&[String]>,
    require_refresh: bool,
    now_ms: i64,
) -> Result<TokenGrant> {
    let scopes: Vec<String> = match (data.get("scope"), previous_scopes) {
        (Some(Value::String(scope)), _) => scope.split_whitespace().map(str::to_owned).collect(),
        (None, Some(previous)) => previous.to_vec(),
        _ => return Err(incomplete_credentials()),
    };
    let string = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
    };
    let access = string("access_token").ok_or_else(incomplete_credentials)?;
    if !string("token_type").is_some_and(|t| t.eq_ignore_ascii_case("bearer")) {
        return Err(incomplete_credentials());
    }
    let expires_in = data
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|seconds| (1..=30 * 24 * 3600).contains(seconds))
        .ok_or_else(incomplete_credentials)?;
    let refresh = string("refresh_token");
    if refresh.is_none() && (require_refresh || scopes.iter().any(|s| s == "offline_access")) {
        return Err(incomplete_credentials());
    }
    let id_token = match data.get("id_token") {
        None | Some(Value::Null) => None,
        Some(Value::String(token)) if !token.is_empty() => Some(Secret::new(token.as_str())),
        Some(_) => return Err(Error::invalid_id_token()),
    };
    Ok(TokenGrant {
        access: Secret::new(access),
        refresh: refresh.map(Secret::new),
        id_token,
        scopes,
        expires_at: now_ms + expires_in * 1000,
        earliest_refresh_at: data.get("earliest_refresh_at").and_then(parse_timestamp_ms),
    })
}

/// `earliest_refresh_at` may be Unix seconds or an RFC 3339 string.
fn parse_timestamp_ms(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_f64().map(|seconds| (seconds * 1000.0) as i64),
        Value::String(text) => {
            let parsed =
                time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                    .ok()?;
            Some((parsed.unix_timestamp_nanos() / 1_000_000) as i64)
        }
        _ => None,
    }
}

/// Form-encoded POST to the token endpoint. No client secret.
pub async fn token_request(http: &Client, endpoint: &Url, form: &[(&str, &str)]) -> Result<Value> {
    let response = http
        .post(endpoint.clone())
        .header(ACCEPT, "application/json")
        .form(form)
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|_| Error::network())?;
    let status = response.status();
    let request_id = header(&response, "x-request-id");
    let body: Option<Value> = response
        .bytes()
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    if !status.is_success() {
        return Err(api_error(
            body.as_ref(),
            Some(status.as_u16()),
            request_id.as_deref(),
        ));
    }
    body.filter(Value::is_object).ok_or_else(|| {
        Error::new(
            ErrorKind::Temporary,
            "invalid_token_response",
            "ChatGPT returned an unexpected sign-in response. Try again.",
        )
    })
}

pub fn header(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

pub struct CodeExchange<'a> {
    pub client_id: &'a str,
    pub code: &'a Secret,
    pub verifier: &'a Secret,
    pub redirect_uri: &'a str,
}

pub async fn exchange_code(
    http: &Client,
    discovery: &Discovery,
    exchange: &CodeExchange<'_>,
    now_ms: i64,
) -> Result<TokenGrant> {
    let form = [
        ("grant_type", "authorization_code"),
        ("client_id", exchange.client_id),
        ("code", exchange.code.expose()),
        ("code_verifier", exchange.verifier.expose()),
        ("redirect_uri", exchange.redirect_uri),
        ("resource", RESOURCE),
    ];
    let data = token_request(http, &discovery.token_endpoint, &form)
        .await
        .map_err(|error| {
            if error.code == "invalid_grant" {
                // The code expired or was used. The issued client ID is already saved, so trying
                // again reuses the registration.
                Error::new(
                    ErrorKind::SignIn,
                    "invalid_grant",
                    "ChatGPT didn't accept this sign-in (the code expired or was already used). \
                 Sign in again.",
                )
            } else {
                error
            }
        })?;
    parse_token_response(&data, None, false, now_ms)
}

/// Revokes the refresh token (empty 200 = success). Retries once on network errors and 5xx.
pub async fn revoke(http: &Client, endpoint: &Url, client_id: &str, refresh: &Secret) -> bool {
    let form = [
        ("token", refresh.expose()),
        ("token_type_hint", "refresh_token"),
        ("client_id", client_id),
    ];
    for attempt in 0..2 {
        let retryable = match http
            .post(endpoint.clone())
            .form(&form)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
        {
            Ok(response) if response.status().as_u16() == 200 => return true,
            Ok(response) => response.status().is_server_error(),
            Err(_) => true,
        };
        if !retryable || attempt == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const STATE: &str = "state-abc";

    fn endpoint() -> Url {
        Url::parse("https://auth.openai.com/api/accounts/authorize").unwrap()
    }

    fn request<'a>(
        issued: Option<&'a str>,
        hint: Option<&'a str>,
        reconsent: bool,
    ) -> AuthorizeRequest<'a> {
        AuthorizeRequest {
            issued_client_id: issued,
            host_id: "urn:uuid:6f1c5c1e-8b4e-4a8e-9f1e-0d6c2a3b4c5d",
            redirect_uri: "http://127.0.0.1:54321/auth/callback",
            state: STATE,
            nonce: "nonce-xyz",
            code_challenge: "challenge",
            login_hint: hint,
            reconsent,
        }
    }

    fn params(url: &Url) -> Vec<(String, String)> {
        url.query_pairs().into_owned().collect()
    }

    fn get<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn pkce_matches_rfc_7636_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn random_tokens_are_fresh_and_url_safe() {
        let (a, b) = (random_token(), random_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn first_registration_url() {
        let url = authorize_url(&endpoint(), &request(None, None, false));
        let p = params(&url);
        assert_eq!(
            url.origin().ascii_serialization(),
            "https://auth.openai.com"
        );
        assert_eq!(url.path(), "/api/accounts/authorize");
        assert_eq!(get(&p, "client_id"), Some(DYNAMIC_CLIENT_ID));
        assert_eq!(get(&p, "agent_name_hint"), Some("Readback"));
        assert_eq!(
            get(&p, "ext_agent_host_id"),
            Some("urn:uuid:6f1c5c1e-8b4e-4a8e-9f1e-0d6c2a3b4c5d")
        );
        assert_eq!(get(&p, "response_type"), Some("code"));
        assert_eq!(
            get(&p, "redirect_uri"),
            Some("http://127.0.0.1:54321/auth/callback")
        );
        assert_eq!(get(&p, "scope"), Some(SCOPES));
        assert_eq!(get(&p, "resource"), Some(RESOURCE));
        assert_eq!(get(&p, "state"), Some(STATE));
        assert_eq!(get(&p, "nonce"), Some("nonce-xyz"));
        assert_eq!(get(&p, "code_challenge_method"), Some("S256"));
        assert_eq!(get(&p, "code_challenge"), Some("challenge"));
        assert_eq!(get(&p, "login_hint"), None);
        assert_eq!(get(&p, "prompt"), None);
        assert_eq!(get(&p, "id_token_hint"), None);
    }

    #[test]
    fn returning_registration_url() {
        let url = authorize_url(
            &endpoint(),
            &request(Some("oaiapp_123"), Some("me@example.com"), false),
        );
        let p = params(&url);
        assert_eq!(get(&p, "client_id"), Some("oaiapp_123"));
        assert_eq!(get(&p, "agent_name_hint"), None);
        assert!(get(&p, "ext_agent_host_id").is_some());
        assert_eq!(get(&p, "login_hint"), Some("me@example.com"));
        assert_eq!(get(&p, "id_token_hint"), None);
        assert_eq!(get(&p, "prompt"), None);
        assert!(url.as_str().contains("login_hint=me%40example.com"));
    }

    #[test]
    fn reconsent_adds_prompt_consent() {
        let url = authorize_url(&endpoint(), &request(Some("oaiapp_123"), None, true));
        assert_eq!(get(&params(&url), "prompt"), Some("consent"));
    }

    fn done(result: CallbackResult) -> Result<Callback> {
        match result {
            CallbackResult::Done(result) => result,
            CallbackResult::InvalidState => panic!("expected a finished callback"),
        }
    }

    #[test]
    fn callback_first_registration() {
        let callback = done(parse_callback(
            "code=abc&scope=openid+chatgpt.tokens.use.direct&state=state-abc&client_id=oaiapp_new",
            STATE,
            None,
        ))
        .unwrap();
        assert_eq!(callback.client_id, "oaiapp_new");
        assert_eq!(callback.code.expose(), "abc");
    }

    #[test]
    fn callback_state_must_match_exactly_once() {
        assert!(matches!(
            parse_callback("code=abc&state=wrong", STATE, None),
            CallbackResult::InvalidState
        ));
        assert!(matches!(
            parse_callback("code=abc", STATE, None),
            CallbackResult::InvalidState
        ));
        assert!(matches!(
            parse_callback("code=abc&state=state-abc&state=state-abc", STATE, None),
            CallbackResult::InvalidState
        ));
        assert!(matches!(
            parse_callback("code=abc&state=state-ab", STATE, None),
            CallbackResult::InvalidState
        ));
    }

    #[test]
    fn callback_access_denied() {
        let error = done(parse_callback(
            "error=access_denied&state=state-abc",
            STATE,
            None,
        ))
        .unwrap_err();
        assert_eq!(error.code, "access_denied");
        assert_eq!(error.kind, ErrorKind::SignIn);
        // A denial with a wrong state is not ours and doesn't stop the pending sign-in.
        assert!(matches!(
            parse_callback("error=access_denied&state=nope", STATE, None),
            CallbackResult::InvalidState
        ));
    }

    #[test]
    fn callback_new_registration_without_client_id_is_incomplete() {
        let error = done(parse_callback("code=abc&state=state-abc", STATE, None)).unwrap_err();
        assert_eq!(error.code, "registration_incomplete");
    }

    #[test]
    fn callback_never_accepts_dynamic_client_id() {
        let error = done(parse_callback(
            "code=abc&state=state-abc&client_id=dynamic_agent_client",
            STATE,
            None,
        ))
        .unwrap_err();
        assert_eq!(error.code, "registration_incomplete");
    }

    #[test]
    fn callback_returning_may_omit_client_id() {
        let callback = done(parse_callback(
            "code=abc&state=state-abc",
            STATE,
            Some("oaiapp_saved"),
        ))
        .unwrap();
        assert_eq!(callback.client_id, "oaiapp_saved");
    }

    #[test]
    fn callback_rejects_mismatched_client_id() {
        let error = done(parse_callback(
            "code=abc&state=state-abc&client_id=oaiapp_other",
            STATE,
            Some("oaiapp_saved"),
        ))
        .unwrap_err();
        assert_eq!(error.code, "client_id_mismatch");
    }

    fn head(target: &str, host: &str, method: &str) -> String {
        format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: test")
    }

    #[test]
    fn listener_only_accepts_its_exact_callback() {
        let ok = head(
            "/auth/callback?code=c&state=state-abc&client_id=oaiapp_1",
            "127.0.0.1:5000",
            "GET",
        );
        let (response, outcome) = handle_request(&ok, 5000, STATE, None);
        assert!(
            String::from_utf8(response)
                .unwrap()
                .starts_with("HTTP/1.1 200")
        );
        assert_eq!(outcome.unwrap().unwrap().client_id, "oaiapp_1");

        for bad in [
            head(
                "/auth/callback?code=c&state=state-abc",
                "localhost:5000",
                "GET",
            ),
            head(
                "/auth/callback?code=c&state=state-abc",
                "127.0.0.1:5001",
                "GET",
            ),
            head("/callback?code=c&state=state-abc", "127.0.0.1:5000", "GET"),
            head(
                "/auth/callback?code=c&state=state-abc",
                "127.0.0.1:5000",
                "POST",
            ),
            head("http://evil/auth/callback", "127.0.0.1:5000", "GET"),
        ] {
            let (response, outcome) = handle_request(&bad, 5000, STATE, None);
            assert!(
                String::from_utf8(response)
                    .unwrap()
                    .starts_with("HTTP/1.1 404"),
                "{bad}"
            );
            assert!(outcome.is_none());
        }

        let wrong_state = head("/auth/callback?code=c&state=other", "127.0.0.1:5000", "GET");
        let (response, outcome) = handle_request(&wrong_state, 5000, STATE, None);
        assert!(
            String::from_utf8(response)
                .unwrap()
                .starts_with("HTTP/1.1 400")
        );
        assert!(outcome.is_none());
    }

    #[test]
    fn discovery_endpoints_must_stay_on_issuer() {
        let mut body = json!({
            "issuer": "https://auth.openai.com",
            "authorization_endpoint": "https://auth.openai.com/api/accounts/authorize",
            "token_endpoint": "https://auth.openai.com/api/accounts/oauth/token",
            "revocation_endpoint": "https://auth.openai.com/api/accounts/oauth/revoke",
            "jwks_uri": "https://auth.openai.com/.well-known/jwks.json",
        });
        let discovery = parse_discovery(&body).unwrap();
        assert_eq!(
            discovery.revocation_endpoint.unwrap().path(),
            "/api/accounts/oauth/revoke"
        );
        body["token_endpoint"] = json!("https://evil.example/token");
        assert!(parse_discovery(&body).is_err());
        body["issuer"] = json!("https://evil.example");
        assert!(parse_discovery(&body).is_err());
    }

    #[test]
    fn token_response_validation() {
        let good = json!({
            "access_token": "a", "refresh_token": "r", "id_token": "i", "token_type": "Bearer",
            "expires_in": 3600, "scope": "openid offline_access chatgpt.tokens.use.direct",
            "earliest_refresh_at": "2026-11-06T12:00:00Z",
        });
        let grant = parse_token_response(&good, None, false, 1_000).unwrap();
        assert_eq!(grant.expires_at, 1_000 + 3_600_000);
        assert_eq!(grant.scopes.len(), 3);
        assert_eq!(grant.earliest_refresh_at, Some(1_793_966_400_000));
        assert!(!format!("{grant:?}").contains("\"a\""));

        let mut missing_refresh = good.clone();
        missing_refresh
            .as_object_mut()
            .unwrap()
            .remove("refresh_token");
        assert!(parse_token_response(&missing_refresh, None, false, 0).is_err());

        let mut missing_scope = good.clone();
        missing_scope.as_object_mut().unwrap().remove("scope");
        assert!(parse_token_response(&missing_scope, None, false, 0).is_err());
        let previous = vec!["openid".to_string()];
        assert_eq!(
            parse_token_response(&missing_scope, Some(&previous), true, 0)
                .unwrap()
                .scopes,
            previous
        );

        let mut wrong_type = good;
        wrong_type["token_type"] = json!("mac");
        assert!(parse_token_response(&wrong_type, None, false, 0).is_err());
    }
}
