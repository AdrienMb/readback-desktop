//! ID token validation: RS256 signature against OpenAI's JWKS, `iss`, `aud` (the issued client
//! ID), `exp`, `nonce`, and `azp` when present. The validated `sub` is the account identity.

use std::time::Duration;

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use reqwest::Client;
use reqwest::header::ACCEPT;
use serde::Deserialize;
use tokio::sync::Mutex;
use url::Url;

use super::errors::{Error, Result};
use super::oauth::ISSUER;
use super::secret::Secret;

const LEEWAY_SECONDS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    pub email: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(#[allow(dead_code)] String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    aud: Audience,
    exp: i64,
    nonce: Option<String>,
    azp: Option<String>,
    email: Option<String>,
}

#[derive(Default)]
pub struct IdTokenVerifier {
    keys: Mutex<Option<JwkSet>>,
}

impl IdTokenVerifier {
    /// `nonce` is checked on sign-in (refreshed ID tokens carry none). `reference_time` is Unix
    /// seconds: "now" on sign-in, the receipt time when resuming a saved refresh.
    pub async fn verify(
        &self,
        http: &Client,
        jwks_uri: &Url,
        token: &Secret,
        client_id: &str,
        nonce: Option<&str>,
        reference_time: i64,
    ) -> Result<Identity> {
        let header = decode_header(token.expose()).map_err(|_| Error::invalid_id_token())?;
        if header.alg != Algorithm::RS256 {
            return Err(Error::invalid_id_token());
        }
        let kid = header.kid.ok_or_else(Error::invalid_id_token)?;
        let key = self.key(http, jwks_uri, &kid).await?;
        verify_with_key(token.expose(), &key, client_id, nonce, reference_time)
    }

    /// Uses the cached key set; refetches once when the key ID is unknown (key rotation).
    /// Failing to get keys is "verification unavailable", not "invalid identity".
    async fn key(&self, http: &Client, jwks_uri: &Url, kid: &str) -> Result<DecodingKey> {
        let mut cache = self.keys.lock().await;
        if let Some(jwk) = cache.as_ref().and_then(|set| set.find(kid)) {
            return DecodingKey::from_jwk(jwk).map_err(|_| Error::invalid_id_token());
        }
        let set = fetch_jwks(http, jwks_uri).await?;
        let key = set.find(kid).map(DecodingKey::from_jwk);
        *cache = Some(set);
        match key {
            Some(Ok(key)) => Ok(key),
            Some(Err(_)) => Err(Error::invalid_id_token()),
            None => Err(Error::identity_unavailable()),
        }
    }
}

async fn fetch_jwks(http: &Client, jwks_uri: &Url) -> Result<JwkSet> {
    let response = http
        .get(jwks_uri.clone())
        .header(ACCEPT, "application/json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| Error::identity_unavailable())?;
    if response.status().as_u16() != 200 {
        return Err(Error::identity_unavailable());
    }
    response
        .json::<JwkSet>()
        .await
        .map_err(|_| Error::identity_unavailable())
}

pub(crate) fn verify_with_key(
    token: &str,
    key: &DecodingKey,
    client_id: &str,
    nonce: Option<&str>,
    reference_time: i64,
) -> Result<Identity> {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["iss", "aud", "exp", "iat", "sub"]);
    validation.leeway = LEEWAY_SECONDS;
    // Expiry is checked below against `reference_time`, which may be in the past.
    validation.validate_exp = false;
    let claims = decode::<Claims>(token, key, &validation)
        .map_err(|_| Error::invalid_id_token())?
        .claims;

    let expired = claims.exp + LEEWAY_SECONDS as i64 <= reference_time;
    let nonce_ok = nonce.is_none_or(|expected| claims.nonce.as_deref() == Some(expected));
    let azp_ok = claims.azp.as_deref().is_none_or(|azp| azp == client_id);
    let multi_aud_ok = match &claims.aud {
        Audience::Many(list) if list.len() > 1 => claims.azp.as_deref() == Some(client_id),
        _ => true,
    };
    if expired || claims.sub.is_empty() || !nonce_ok || !azp_ok || !multi_aud_ok {
        return Err(Error::invalid_id_token());
    }
    Ok(Identity {
        subject: claims.sub,
        email: claims.email,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::{Value, json};

    const PRIVATE_KEY: &str = include_str!("testdata/test_rsa.pem");
    const MODULUS: &str = include_str!("testdata/test_rsa_n.txt");
    const NOW: i64 = 1_800_000_000;

    fn sign(claims: Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-key".into());
        encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn key() -> DecodingKey {
        DecodingKey::from_rsa_components(MODULUS.trim(), "AQAB").unwrap()
    }

    fn claims() -> Value {
        json!({
            "iss": ISSUER, "aud": "oaiapp_1", "sub": "user-1", "email": "me@example.com",
            "iat": NOW - 10, "exp": NOW + 3600, "nonce": "n-1",
        })
    }

    fn check(claims: Value, nonce: Option<&str>) -> Result<Identity> {
        verify_with_key(&sign(claims), &key(), "oaiapp_1", nonce, NOW)
    }

    #[test]
    fn valid_token_yields_identity() {
        let identity = check(claims(), Some("n-1")).unwrap();
        assert_eq!(
            identity,
            Identity {
                subject: "user-1".into(),
                email: Some("me@example.com".into())
            }
        );
    }

    #[test]
    fn rejects_bad_claims() {
        let cases: Vec<(&str, Value)> = vec![
            ("aud", json!("oaiapp_other")),
            ("iss", json!("https://evil.example")),
            ("exp", json!(NOW - 60)),
            ("azp", json!("oaiapp_other")),
            ("aud", json!(["oaiapp_1", "other"])),
        ];
        for (field, value) in cases {
            let mut claims = claims();
            claims[field] = value;
            assert_eq!(
                check(claims, Some("n-1")).unwrap_err().code,
                "invalid_id_token",
                "{field}"
            );
        }
    }

    #[test]
    fn nonce_must_match_on_sign_in() {
        assert!(check(claims(), Some("other")).is_err());
        let mut no_nonce = claims();
        no_nonce.as_object_mut().unwrap().remove("nonce");
        assert!(check(no_nonce.clone(), Some("n-1")).is_err());
        // Refreshed ID tokens are checked without a nonce.
        assert!(check(no_nonce, None).is_ok());
    }

    #[test]
    fn rejects_tampered_signature() {
        let token = sign(claims());
        let mut forged_claims = claims();
        forged_claims["sub"] = json!("attacker");
        let forged = sign(forged_claims);
        // Original header and signature with another payload.
        let mut parts: Vec<String> = token.split('.').map(str::to_owned).collect();
        parts[1] = forged.split('.').nth(1).unwrap().to_owned();
        assert!(verify_with_key(&parts.join("."), &key(), "oaiapp_1", None, NOW).is_err());
    }
}
