use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use ptrack_capability_policy::{AuditEvent, Denied, authorize_http};
use ptrack_core::Capability;
use ptrack_store::{Clock, ProjectStore, SystemClock};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::AuditRecorder;

const MAX_TRANSIENT_HEADER_BYTES: usize = 64 << 10;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty", with = "base64_bytes")]
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status_code: u16,
    pub status: String,
    pub headers: BTreeMap<String, Vec<String>>,
    #[serde(with = "base64_bytes")]
    pub body: Vec<u8>,
    pub effective_url: String,
    pub redirects: i64,
    pub diagnostics: HttpDiagnostics,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HttpDiagnostics {
    pub proxy: String,
    pub ca_store: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionClass {
    Cancelled,
    Denied,
    Dns,
    Proxy,
    ResponseLimit,
    Routing,
    Sandbox,
    Timeout,
    Tls,
    Transport,
}

impl ConnectionClass {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Denied => "denied",
            Self::Dns => "dns",
            Self::Proxy => "proxy",
            Self::ResponseLimit => "response-limit",
            Self::Routing => "routing",
            Self::Sandbox => "sandbox",
            Self::Timeout => "timeout",
            Self::Tls => "tls",
            Self::Transport => "transport",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpError {
    message: String,
    class: ConnectionClass,
    status_code: u16,
    diagnostics: HttpDiagnostics,
}

impl HttpError {
    fn new(message: impl Into<String>, class: ConnectionClass) -> Self {
        Self {
            message: message.into(),
            class,
            status_code: 0,
            diagnostics: HttpDiagnostics {
                proxy: String::new(),
                ca_store: String::new(),
            },
        }
    }

    fn with_response_metadata(mut self, status_code: u16, diagnostics: HttpDiagnostics) -> Self {
        self.status_code = status_code;
        self.diagnostics = diagnostics;
        self
    }

    #[must_use]
    pub const fn class(&self) -> ConnectionClass {
        self.class
    }

    pub(crate) const fn status_code(&self) -> u16 {
        self.status_code
    }

    pub(crate) const fn diagnostics(&self) -> &HttpDiagnostics {
        &self.diagnostics
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HttpError {}

impl From<Denied> for HttpError {
    fn from(error: Denied) -> Self {
        Self::new(error.to_string(), ConnectionClass::Denied)
    }
}

pub struct HttpExecutor<'a> {
    pub(crate) recorder: AuditRecorder<'a>,
}

impl<'a> HttpExecutor<'a> {
    #[must_use]
    pub const fn new(store: Option<&'a ProjectStore>) -> Self {
        Self {
            recorder: AuditRecorder::new(store),
        }
    }

    pub(crate) const fn from_recorder(recorder: AuditRecorder<'a>) -> Self {
        Self { recorder }
    }

    /// Executes one manually redirected, fully authorized HTTP exchange.
    ///
    /// # Errors
    /// Returns only stable policy or diagnostic classes; transport internals,
    /// URLs, headers, and bodies are never included.
    pub async fn execute(
        &self,
        cancellation: &CancellationToken,
        capability: &Capability,
        agent_profile: &str,
        request: &HttpRequest,
    ) -> Result<HttpResponse, HttpError> {
        let now = SystemClock.now_utc();
        let request_bytes = i64::try_from(request.body.len()).unwrap_or(i64::MAX);
        let (normalized, authorized_url) = authorize_http(
            capability,
            agent_profile,
            now,
            &request.method,
            &request.url,
            request_bytes,
        )?;
        let headers = validate_headers(&request.headers)?;
        let timeout = Duration::from_secs(
            u64::try_from(normalized.limits.timeout_seconds).unwrap_or_default(),
        );
        let started = Instant::now();
        let exchange = self.exchange(
            cancellation,
            &normalized,
            agent_profile,
            request,
            headers,
            authorized_url,
        );
        let outcome = tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(HttpError::new("HTTP request cancelled", ConnectionClass::Cancelled)),
            result = tokio::time::timeout(timeout, exchange) => match result {
                Ok(value) => value,
                Err(_) => Err(HttpError::new("HTTP request timed out", ConnectionClass::Timeout)),
            }
        };
        let (response_bytes, redirects) = outcome.as_ref().map_or((0, 0), |response| {
            (
                i64::try_from(response.body.len()).unwrap_or(i64::MAX),
                response.redirects,
            )
        });
        let event = AuditEvent {
            operation: request.method.to_uppercase(),
            target: request.url.clone(),
            success: outcome.is_ok(),
            error_class: outcome.as_ref().err().map_or_else(
                || "none".to_owned(),
                |error| error.class.as_str().to_owned(),
            ),
            duration_millis: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
            request_bytes,
            response_bytes,
            redirects,
        };
        if let Err(error) = self.recorder.record(&normalized, &event)
            && outcome.is_ok()
        {
            return Err(HttpError::new(
                error.to_string(),
                ConnectionClass::Transport,
            ));
        }
        outcome
    }

    async fn exchange(
        &self,
        cancellation: &CancellationToken,
        capability: &Capability,
        agent_profile: &str,
        request: &HttpRequest,
        headers: HeaderMap,
        authorized_url: String,
    ) -> Result<HttpResponse, HttpError> {
        ensure_tls_provider()?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| {
                HttpError::new("HTTP transport is unavailable", ConnectionClass::Transport)
            })?;
        let mut method = parse_method(&request.method)?;
        let mut url = Url::parse(&authorized_url)
            .map_err(|_| HttpError::new("HTTP request URL is invalid", ConnectionClass::Denied))?;
        let mut hop_headers = headers;
        let mut body = request.body.clone();
        let mut redirects = 0_i64;
        let diagnostics = HttpDiagnostics {
            proxy: sanitized_proxy_diagnostic(&url),
            ca_store: "system".to_owned(),
        };
        loop {
            let send = client
                .request(method.clone(), url.clone())
                .headers(hop_headers.clone())
                .body(body.clone())
                .send();
            let response = tokio::select! {
                biased;
                () = cancellation.cancelled() => return Err(HttpError::new("HTTP request cancelled", ConnectionClass::Cancelled)),
                result = send => result.map_err(|error| classify_reqwest(&error))?,
            };
            if response.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
                return Err(HttpError::new(
                    "HTTP proxy requires authentication",
                    ConnectionClass::Proxy,
                )
                .with_response_metadata(response.status().as_u16(), diagnostics));
            }
            ensure_response_headers_bounded(response.headers())?;
            if response.status().is_redirection() {
                let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
                    return finish_response(response, redirects, diagnostics, capability).await;
                };
                redirects += 1;
                if redirects > capability.limits.max_redirects {
                    return Err(HttpError::new(
                        "capability denied: HTTP redirect limit exceeded",
                        ConnectionClass::Denied,
                    ));
                }
                let location = location.to_str().map_err(|_| {
                    HttpError::new("HTTP redirect URL is invalid", ConnectionClass::Denied)
                })?;
                let next = url.join(location).map_err(|_| {
                    HttpError::new("HTTP redirect URL is invalid", ConnectionClass::Denied)
                })?;
                if matches!(response.status(), StatusCode::SEE_OTHER)
                    || (matches!(
                        response.status(),
                        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND
                    ) && method == Method::POST)
                {
                    method = Method::GET;
                    body.clear();
                    hop_headers.remove(reqwest::header::CONTENT_LENGTH);
                    hop_headers.remove(reqwest::header::CONTENT_TYPE);
                }
                authorize_http(
                    capability,
                    agent_profile,
                    SystemClock.now_utc(),
                    method.as_str(),
                    next.as_str(),
                    i64::try_from(request.body.len()).unwrap_or(i64::MAX),
                )
                .map_err(|error| {
                    HttpError::new(
                        format!("redirect rejected: {error}"),
                        ConnectionClass::Denied,
                    )
                })?;
                strip_sensitive_headers(&mut hop_headers);
                url = next;
                continue;
            }
            return finish_response(response, redirects, diagnostics, capability).await;
        }
    }
}

pub(crate) fn ensure_tls_provider() -> Result<(), HttpError> {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *INSTALLED.get_or_init(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .is_ok()
            || rustls::crypto::CryptoProvider::get_default().is_some()
    }) {
        Ok(())
    } else {
        Err(HttpError::new(
            "HTTP transport is unavailable",
            ConnectionClass::Transport,
        ))
    }
}

async fn finish_response(
    response: reqwest::Response,
    redirects: i64,
    diagnostics: HttpDiagnostics,
    capability: &Capability,
) -> Result<HttpResponse, HttpError> {
    let status_code = response.status().as_u16();
    let status = response.status().canonical_reason().map_or_else(
        || status_code.to_string(),
        |reason| format!("{status_code} {reason}"),
    );
    let headers = response_headers(response.headers());
    let effective_url = response.url().to_string();
    let limit = usize::try_from(capability.limits.max_response_bytes).unwrap_or(usize::MAX);
    let mut response = response;
    let mut body = Vec::with_capacity(limit.min(8_192));
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| classify_reqwest(&error))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(HttpError::new(
                "HTTP response exceeds its byte limit",
                ConnectionClass::ResponseLimit,
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(HttpResponse {
        status_code,
        status,
        headers,
        body,
        effective_url,
        redirects,
        diagnostics,
    })
}

fn parse_method(raw: &str) -> Result<Method, HttpError> {
    Method::from_bytes(raw.trim().to_uppercase().as_bytes())
        .map_err(|_| HttpError::new("HTTP method is invalid", ConnectionClass::Denied))
}

pub(crate) fn validate_headers(
    headers: &BTreeMap<String, Vec<String>>,
) -> Result<HeaderMap, HttpError> {
    let mut output = HeaderMap::new();
    let mut total = 0_usize;
    for (name, values) in headers {
        if name.contains(['\r', '\n']) || is_forbidden_header(name) {
            return Err(HttpError::new(
                format!("HTTP header {name:?} is not allowed"),
                ConnectionClass::Denied,
            ));
        }
        let parsed_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            HttpError::new(
                format!("HTTP header {name:?} is not allowed"),
                ConnectionClass::Denied,
            )
        })?;
        for value in values {
            if value.contains(['\r', '\n']) {
                return Err(HttpError::new(
                    format!("HTTP header {name:?} contains a newline"),
                    ConnectionClass::Denied,
                ));
            }
            total = total.saturating_add(name.len()).saturating_add(value.len());
            if total > MAX_TRANSIENT_HEADER_BYTES {
                return Err(HttpError::new(
                    "HTTP headers exceed their byte limit",
                    ConnectionClass::Denied,
                ));
            }
            let parsed_value = HeaderValue::from_str(value).map_err(|_| {
                HttpError::new(
                    format!("HTTP header {name:?} is not allowed"),
                    ConnectionClass::Denied,
                )
            })?;
            output.append(parsed_name.clone(), parsed_value);
        }
    }
    Ok(output)
}

fn is_forbidden_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "proxy-authorization"
            | "connection"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn strip_sensitive_headers(headers: &mut HeaderMap) {
    headers.remove(reqwest::header::AUTHORIZATION);
    headers.remove(reqwest::header::COOKIE);
    headers.remove("proxy-authorization");
}

fn ensure_response_headers_bounded(headers: &HeaderMap) -> Result<(), HttpError> {
    let total = headers.iter().fold(0_usize, |total, (name, value)| {
        total
            .saturating_add(name.as_str().len())
            .saturating_add(value.as_bytes().len())
    });
    if total > MAX_TRANSIENT_HEADER_BYTES {
        return Err(HttpError::new(
            "HTTP response headers exceed their byte limit",
            ConnectionClass::ResponseLimit,
        ));
    }
    Ok(())
}

fn response_headers(headers: &HeaderMap) -> BTreeMap<String, Vec<String>> {
    let mut output: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in headers {
        let value = value
            .to_str()
            .map_or_else(|_| String::new(), ToOwned::to_owned);
        output.entry(name.to_string()).or_default().push(value);
    }
    output
}

fn sanitized_proxy_diagnostic(target: &Url) -> String {
    proxy_diagnostic_from(target, |name| std::env::var(name).ok())
}

pub(crate) fn proxy_diagnostic_from(
    target: &Url,
    lookup: impl Fn(&str) -> Option<String>,
) -> String {
    let no_proxy = lookup("NO_PROXY").or_else(|| lookup("no_proxy"));
    if no_proxy
        .as_deref()
        .is_some_and(|rules| proxy_bypassed(target, rules))
    {
        return "direct".to_owned();
    }
    let raw = if target.scheme() == "https" {
        lookup("HTTPS_PROXY").or_else(|| lookup("https_proxy"))
    } else if lookup("REQUEST_METHOD").is_some() {
        lookup("http_proxy")
    } else {
        lookup("HTTP_PROXY").or_else(|| lookup("http_proxy"))
    };
    let Some(raw) = raw else {
        return "direct".to_owned();
    };
    let Ok(mut proxy) = Url::parse(&raw) else {
        return "error".to_owned();
    };
    let _ = proxy.set_username("");
    let _ = proxy.set_password(None);
    proxy.set_query(None);
    proxy.set_fragment(None);
    proxy.to_string().trim_end_matches('/').to_owned()
}

fn proxy_bypassed(target: &Url, rules: &str) -> bool {
    let Some(host) = target.host_str() else {
        return false;
    };
    let port = target.port_or_known_default();
    rules.split(',').map(str::trim).any(|rule| {
        if rule == "*" {
            return true;
        }
        if rule.is_empty() {
            return false;
        }
        let rule = rule.split('/').next().unwrap_or(rule);
        let (rule_host, rule_port) = split_proxy_rule(rule);
        if rule_port.is_some() && rule_port != port {
            return false;
        }
        let rule_host = rule_host.trim_start_matches('.');
        host.eq_ignore_ascii_case(rule_host)
            || host
                .to_ascii_lowercase()
                .ends_with(&format!(".{}", rule_host.to_ascii_lowercase()))
    })
}

fn split_proxy_rule(rule: &str) -> (&str, Option<u16>) {
    if let Some(bracketed) = rule.strip_prefix('[')
        && let Some((host, suffix)) = bracketed.split_once(']')
    {
        return (
            host,
            suffix.strip_prefix(':').and_then(|port| port.parse().ok()),
        );
    }
    if rule.matches(':').count() == 1
        && let Some((host, port)) = rule.rsplit_once(':')
        && let Ok(port) = port.parse()
    {
        return (host, Some(port));
    }
    (rule, None)
}

fn classify_reqwest(error: &reqwest::Error) -> HttpError {
    if error.is_timeout() {
        return HttpError::new("HTTP request timed out", ConnectionClass::Timeout);
    }
    let lower = error.to_string().to_ascii_lowercase();
    let class = if lower.contains("dns") || lower.contains("resolve") {
        ConnectionClass::Dns
    } else if lower.contains("certificate") || lower.contains("tls") {
        ConnectionClass::Tls
    } else if lower.contains("permission denied") || lower.contains("operation not permitted") {
        ConnectionClass::Sandbox
    } else if lower.contains("connection refused")
        || lower.contains("network is unreachable")
        || lower.contains("no route to host")
    {
        ConnectionClass::Routing
    } else {
        ConnectionClass::Transport
    };
    HttpError::new("HTTP transport failed", class)
}

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(value))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        STANDARD.decode(value).map_err(serde::de::Error::custom)
    }
}
