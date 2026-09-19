// Two mutually-exclusive auth backends, decided once at startup from env vars:
//   - Password: a single shared password compared against `Authorization: Bearer`.
//   - Oidc: a real OIDC provider (discovered via `.well-known/openid-configuration`),
//     with the app issuing its own opaque session cookie after a successful login.
// If OIDC_ISSUER_URL is set it forces Oidc mode; otherwise AUTH_PASSWORD (or no
// auth at all) behaves exactly as before.

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json, Redirect, Response},
};
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, TokenResponse,
};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::AppState;

type OidcClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

const SESSION_COOKIE: &str = "session";
const SESSION_TTL: Duration = Duration::from_secs(60 * 60 * 12);
const PENDING_TTL: Duration = Duration::from_secs(60 * 10);

pub(crate) struct OidcState {
    client: OidcClient,
    secure_cookie: bool,
    // Opaque session token -> expiry.
    sessions: Mutex<HashMap<String, Instant>>,
    // CSRF state token -> (PKCE verifier, nonce, expiry), alive only between
    // the login redirect and the callback.
    pending: Mutex<HashMap<String, (PkceCodeVerifier, Nonce, Instant)>>,
}

pub(crate) enum AuthMode {
    None,
    Password(String),
    Oidc(Box<OidcState>),
}

#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    pub password: String,
}

#[derive(Serialize)]
pub(crate) struct VerifyResponse {
    authed: bool,
    mode: &'static str,
}

pub(crate) async fn build_auth_mode(client: &reqwest::Client) -> AuthMode {
    let issuer = std::env::var("OIDC_ISSUER_URL")
        .ok()
        .filter(|s| !s.is_empty());

    let Some(issuer) = issuer else {
        let auth_password = std::env::var("AUTH_PASSWORD")
            .ok()
            .filter(|s| !s.is_empty());
        if auth_password.is_none() {
            tracing::warn!("AUTH_PASSWORD not set — status updates are unauthenticated");
        }
        return match auth_password {
            Some(pw) => AuthMode::Password(pw),
            None => AuthMode::None,
        };
    };

    let client_id = std::env::var("OIDC_CLIENT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .expect("OIDC_CLIENT_ID must be set when OIDC_ISSUER_URL is set");
    let client_secret = std::env::var("OIDC_CLIENT_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .expect("OIDC_CLIENT_SECRET must be set when OIDC_ISSUER_URL is set");
    let redirect_url = std::env::var("OIDC_REDIRECT_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .expect("OIDC_REDIRECT_URL must be set when OIDC_ISSUER_URL is set");

    let secure_cookie = redirect_url.starts_with("https://");

    let issuer_url = IssuerUrl::new(issuer)
        .unwrap_or_else(|e| panic!("OIDC_ISSUER_URL is not a valid URL: {e}"));
    let metadata = CoreProviderMetadata::discover_async(issuer_url, client)
        .await
        .unwrap_or_else(|e| panic!("Failed to discover OIDC provider metadata: {e}"));

    let redirect_url = RedirectUrl::new(redirect_url)
        .unwrap_or_else(|e| panic!("OIDC_REDIRECT_URL is not a valid URL: {e}"));

    let oidc_client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(client_id),
        Some(ClientSecret::new(client_secret)),
    )
    .set_redirect_uri(redirect_url);

    tracing::info!("OIDC auth enabled (issuer configured via OIDC_ISSUER_URL)");

    AuthMode::Oidc(Box::new(OidcState {
        client: oidc_client,
        secure_cookie,
        sessions: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
    }))
}

pub(crate) fn check_auth(mode: &AuthMode, headers: &HeaderMap) -> bool {
    match mode {
        AuthMode::None => true,
        AuthMode::Password(pw) => bearer_token(headers).map(|t| t == pw).unwrap_or(false),
        AuthMode::Oidc(state) => session_token_from_cookie(headers)
            .map(|t| is_valid_session(state, t))
            .unwrap_or(false),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

fn session_token_from_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|kv| kv.strip_prefix("session="))
}

fn is_valid_session(state: &OidcState, token: &str) -> bool {
    let mut sessions = state.sessions.lock().unwrap();
    prune_expired(&mut sessions);
    sessions.contains_key(token)
}

fn prune_expired(map: &mut HashMap<String, Instant>) {
    let now = Instant::now();
    map.retain(|_, expires| *expires > now);
}

fn prune_pending(map: &mut HashMap<String, (PkceCodeVerifier, Nonce, Instant)>) {
    let now = Instant::now();
    map.retain(|_, (_, _, expires)| *expires > now);
}

pub(crate) async fn verify_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Json<VerifyResponse> {
    let mode = match state.auth.as_ref() {
        AuthMode::None => "none",
        AuthMode::Password(_) => "password",
        AuthMode::Oidc(_) => "oidc",
    };
    let authed = check_auth(&state.auth, &headers);
    Json(VerifyResponse { authed, mode })
}

pub(crate) async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    match state.auth.as_ref() {
        AuthMode::None => StatusCode::OK.into_response(),
        AuthMode::Password(pw) if *pw == body.password => StatusCode::OK.into_response(),
        AuthMode::Password(_) => StatusCode::UNAUTHORIZED.into_response(),
        // Password login is meaningless once OIDC is forced.
        AuthMode::Oidc(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

pub(crate) async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let AuthMode::Oidc(oidc) = state.auth.as_ref() else {
        return StatusCode::NO_CONTENT.into_response();
    };

    if let Some(token) = session_token_from_cookie(&headers) {
        oidc.sessions.lock().unwrap().remove(token);
    }

    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, expire_cookie_header(oidc.secure_cookie));
    response
}

pub(crate) async fn oidc_login(State(state): State<AppState>) -> Response {
    let AuthMode::Oidc(oidc) = state.auth.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_token, nonce) = oidc
        .client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_pkce_challenge(pkce_challenge)
        .url();

    {
        let mut pending = oidc.pending.lock().unwrap();
        prune_pending(&mut pending);
        pending.insert(
            csrf_token.secret().clone(),
            (pkce_verifier, nonce, Instant::now() + PENDING_TTL),
        );
    }

    Redirect::to(auth_url.as_str()).into_response()
}

#[derive(Deserialize)]
pub(crate) struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

pub(crate) async fn oidc_callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
) -> Response {
    let AuthMode::Oidc(oidc) = state.auth.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if let Some(err) = params.error {
        let msg = params.error_description.unwrap_or(err);
        tracing::warn!("OIDC provider returned an error: {msg}");
        return error_redirect(&msg);
    }

    let (Some(code), Some(csrf_state)) = (params.code, params.state) else {
        return error_redirect("Missing code or state from provider");
    };

    let pending_entry = {
        let mut pending = oidc.pending.lock().unwrap();
        prune_pending(&mut pending);
        pending.remove(&csrf_state)
    };
    let Some((pkce_verifier, nonce, _)) = pending_entry else {
        return error_redirect("Login session expired or invalid — please try again");
    };

    let exchange = match oidc.client.exchange_code(AuthorizationCode::new(code)) {
        Ok(req) => req,
        Err(e) => {
            tracing::error!("OIDC exchange_code configuration error: {e}");
            return error_redirect("Login failed");
        }
    };

    let token_response = match exchange
        .set_pkce_verifier(pkce_verifier)
        .request_async(state.client.as_ref())
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("OIDC token exchange failed: {e}");
            return error_redirect("Login failed");
        }
    };

    let Some(id_token) = token_response.id_token() else {
        return error_redirect("Provider did not return an ID token");
    };

    let verifier = oidc.client.id_token_verifier();
    if let Err(e) = id_token.claims(&verifier, &nonce) {
        tracing::error!("OIDC ID token verification failed: {e}");
        return error_redirect("Login failed");
    }

    let session_token = random_token(32);
    {
        let mut sessions = oidc.sessions.lock().unwrap();
        prune_expired(&mut sessions);
        sessions.insert(session_token.clone(), Instant::now() + SESSION_TTL);
    }

    let mut response = Redirect::to("/").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        set_cookie_header(
            &session_token,
            SESSION_TTL.as_secs() as i64,
            oidc.secure_cookie,
        ),
    );
    response
}

fn set_cookie_header(token: &str, max_age_secs: i64, secure: bool) -> HeaderValue {
    let secure_flag = if secure { "; Secure" } else { "" };
    let val = format!(
        "{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_secs}{secure_flag}"
    );
    HeaderValue::from_str(&val).expect("cookie header value should be valid ASCII")
}

fn expire_cookie_header(secure: bool) -> HeaderValue {
    let secure_flag = if secure { "; Secure" } else { "" };
    let val =
        format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure_flag}");
    HeaderValue::from_str(&val).expect("cookie header value should be valid ASCII")
}

fn error_redirect(msg: &str) -> Response {
    Redirect::to(&format!("/?oidc_error={}", url_encode(msg))).into_response()
}

fn random_token(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
