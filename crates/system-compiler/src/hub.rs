//! The compiler side of the hub's compiler protocol: configuration, the
//! WebSocket job channel, section submission, and the `serve` daemon.
//!
//! This module implements `compiler-pipeline.md` sections 5 to 8 and
//! `third-party-api.md` section 10 of the hub's architecture docs, with the
//! exact frame and header shapes taken from the hub's code. The protocol as
//! implemented here, and the file each fact came from, is written down in
//! `docs/protocol.md`. In short:
//!
//! - The compiler opens `GET /compiler/connect` as a WebSocket with the
//!   headers `X-API-Key`, `X-Compiler-Secret`, and `X-Compiler-Id`. The hub
//!   closes the socket with code `4401` when the compiler credentials are
//!   wrong, and refuses the upgrade with HTTP `401` or `403` when the API key
//!   is missing, invalid, or lacks `compile:submit`.
//! - The hub pushes one JSON text frame per job ([`Job`]).
//! - The compiler compiles the job's chunk key with
//!   [`compile`] and posts the bytes to
//!   `POST /space/{spaceId}/build/{buildId}/chunk/{chunkKey}/compiled`.
//!
//! Credentials are held in [`Secret`], whose `Debug` and `Display` never
//! print the value. Nothing in this module logs a credential or puts one in
//! an error message.
//!
//! Reconnect and retry delays come from [`Backoff`]: 1 s doubling to 60 s,
//! with jitter drawn from a seeded generator so that a run's delays, and so
//! its logs, are reproducible.

use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use gx_core::key::ChunkKey;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::HeaderValue;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{error, info, warn};
use url::Url;

use crate::compile::{compile, CompileOptions};
use crate::System;

/// WebSocket close code the hub uses when compiler authentication fails
/// (`CompilerConnectController.cs`, `CloseUnauthorized`).
pub const CLOSE_UNAUTHORIZED: u16 = 4401;

/// Value of the `X-Layer-Section-Type` header on every submission.
pub const SECTION_TYPE: &str = "matter";

/// Default number of jobs compiled and submitted at once.
pub const DEFAULT_WORKERS: usize = 4;

/// Default seed of the jitter generator used for reconnect delays.
pub const DEFAULT_BACKOFF_SEED: u64 = 0;

/// How long the socket may stay silent before it is treated as dropped.
///
/// The hub runs ASP.NET Core's WebSocket middleware with its default
/// keep-alive interval of 120 s, so a live socket carries at least one
/// keep-alive frame every 120 s. Silence for two and a half intervals means
/// the connection is gone even if TCP has not noticed.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Attempts made for one submission that keeps failing with `5xx` or a
/// transport error, before the job is logged as failed.
pub const DEFAULT_SUBMIT_ATTEMPTS: u32 = 8;

/// The environment variables [`HubConfig`] reads, in the order
/// `(primary, fallback)`. The fallbacks are the names the hub's local seed
/// script writes into a compiler's `.env`.
pub const ENV_VARS: [(&str, &str); 4] = [
    ("GX_HUB_URL", "HUB_URL"),
    ("GX_API_KEY", "COMPILER_API_KEY"),
    ("GX_COMPILER_ID", "COMPILER_ID"),
    ("GX_COMPILER_SECRET", "COMPILER_SECRET"),
];

/// A credential value. `Debug` and `Display` print `<redacted>`; the value is
/// only reachable through [`Secret::expose`], which is called when a header
/// is built and nowhere else.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a credential value.
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    /// The raw value, for building a request header.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Where the hub is and how this compiler authenticates to it.
#[derive(Clone, Debug)]
pub struct HubConfig {
    /// Base URL of the hub, `http` or `https`, for example
    /// `http://localhost:5297`. Paths are appended to it.
    pub hub_url: Url,
    /// API key holding `compile:submit`, sent as `X-API-Key`.
    pub api_key: Secret,
    /// The compiler's id as issued by the hub, sent as `X-Compiler-Id`.
    pub compiler_id: String,
    /// The compiler's raw shared secret, sent as `X-Compiler-Secret`.
    pub compiler_secret: Secret,
}

/// Why a [`HubConfig`] could not be built. Messages name variables, never
/// their values.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required variable is unset or empty.
    #[error("environment variable {0} is not set (fallback {1} is not set either)")]
    Missing(&'static str, &'static str),
    /// `GX_HUB_URL` is not an absolute `http` or `https` URL.
    #[error("GX_HUB_URL is not an absolute http or https URL")]
    BadUrl,
    /// The `.env` file exists but could not be read or parsed.
    #[error("environment file {path}: {reason}")]
    EnvFile {
        /// The file.
        path: PathBuf,
        /// What went wrong, without any value from the file.
        reason: String,
    },
}

impl HubConfig {
    /// Reads the configuration from the process environment. See
    /// [`HubConfig::from_lookup`].
    pub fn from_env() -> Result<HubConfig, ConfigError> {
        HubConfig::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads the configuration through `lookup`, which maps a variable name
    /// to its value. Each of [`ENV_VARS`] is read under its primary name,
    /// then its fallback name; empty values count as unset.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<HubConfig, ConfigError> {
        let get = |i: usize| -> Result<String, ConfigError> {
            let (primary, fallback) = ENV_VARS[i];
            [primary, fallback]
                .iter()
                .filter_map(|n| lookup(n))
                .map(|v| v.trim().to_string())
                .find(|v| !v.is_empty())
                .ok_or(ConfigError::Missing(primary, fallback))
        };
        let url = get(0)?;
        let hub_url = Url::parse(url.trim_end_matches('/')).map_err(|_| ConfigError::BadUrl)?;
        if !matches!(hub_url.scheme(), "http" | "https") || hub_url.cannot_be_a_base() {
            return Err(ConfigError::BadUrl);
        }
        Ok(HubConfig {
            hub_url,
            api_key: Secret::new(get(1)?),
            compiler_id: get(2)?,
            compiler_secret: Secret::new(get(3)?),
        })
    }

    /// The WebSocket URL of `GET /compiler/connect`: the hub URL with `http`
    /// mapped to `ws` and `https` to `wss`.
    pub fn connect_url(&self) -> Url {
        let mut url = self.endpoint(&["compiler", "connect"]);
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .expect("ws and wss are valid schemes");
        url
    }

    /// The URL of `POST /space/{spaceId}/build/{buildId}/chunk/{chunkKey}/compiled`,
    /// each path segment percent-encoded.
    pub fn submit_url(&self, space_id: &str, build_id: &str, chunk_key: &str) -> Url {
        self.endpoint(&[
            "space", space_id, "build", build_id, "chunk", chunk_key, "compiled",
        ])
    }

    fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.hub_url.clone();
        url.set_query(None);
        url.set_fragment(None);
        url.path_segments_mut()
            .expect("checked in from_lookup")
            .pop_if_empty()
            .extend(segments);
        url
    }
}

/// Loads a `.env` file into the process environment without overriding
/// variables that are already set, and without logging any value.
///
/// With `path`, that file must exist. Without it, `.env` is searched for in
/// the working directory and its parents, and a missing file is not an
/// error. Returns the file that was loaded, if any.
pub fn load_dotenv(path: Option<&Path>) -> Result<Option<PathBuf>, ConfigError> {
    let loaded = match path {
        Some(p) => dotenvy::from_path(p).map(|()| p.to_path_buf()),
        None => dotenvy::dotenv(),
    };
    match loaded {
        Ok(p) => {
            info!(path = %p.display(), "loaded environment file");
            Ok(Some(p))
        }
        Err(e) if path.is_none() && e.not_found() => Ok(None),
        Err(e) => Err(ConfigError::EnvFile {
            path: path.map_or_else(|| PathBuf::from(".env"), Path::to_path_buf),
            reason: dotenv_reason(&e),
        }),
    }
}

/// Describes a dotenvy error without echoing the offending line, which may
/// hold a credential.
fn dotenv_reason(e: &dotenvy::Error) -> String {
    match e {
        dotenvy::Error::LineParse(_, index) => format!("parse error at byte {index}"),
        dotenvy::Error::Io(io) => io.kind().to_string(),
        _ => "unreadable".to_string(),
    }
}

/// One compilation job, as pushed by the hub in a WebSocket text frame
/// (`WebSocketJobDispatchAdapter.cs`, `DispatchJobAsync`). The ids are GUIDs
/// in their canonical string form; `layerId` is the layer's name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    /// The job's id; unique per dispatch, repeated on re-dispatch.
    pub job_id: String,
    /// The space the chunk belongs to.
    pub space_id: String,
    /// The build the chunk belongs to.
    pub build_id: String,
    /// The chunk key to compile: `registry` or `frameId-depth-x-y-z`.
    pub chunk_key: String,
    /// The compiler the job is addressed to: this compiler.
    pub compiler_id: String,
    /// The layer this compiler owns in the build.
    pub layer_id: String,
}

/// The JSON body of a `400` validation rejection
/// (`Models/SectionRejection.cs`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    /// Code from the core library's error table.
    pub code: i64,
    /// Short name of the code, such as `BAD_MAGIC`.
    pub name: String,
    /// Human readable reason reported by the validator.
    pub reason: String,
}

/// What the hub said about one submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// `201 Created`: the section is stored.
    Created,
    /// `409 Conflict`: a section for this layer was already stored. Treated as
    /// success. `reason` is the hub's `{reason}` body when it sent one (an
    /// ambiguous legacy snapshot), which is worth a warning.
    AlreadyStored {
        /// The hub's reason, if the response had one.
        reason: Option<String>,
    },
    /// `400 Bad Request`: the bytes failed validation (with a body) or the
    /// request was malformed (without one). Never retried.
    Rejected(Option<Rejection>),
    /// Any other status that is neither success, a credential problem, nor a
    /// server error, such as `404`. Not retried.
    Unexpected,
}

/// The result of [`Submitter::submit`]: the final HTTP status and its meaning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    /// HTTP status of the last attempt.
    pub status: u16,
    /// What the status means.
    pub outcome: SubmitOutcome,
    /// Attempts made, 1 when the first one was answered.
    pub attempts: u32,
}

/// Errors talking to the hub. Messages never contain credential values.
#[derive(Debug, thiserror::Error)]
pub enum HubError {
    /// The hub refused this compiler's credentials on the WebSocket: close
    /// code `4401`, or HTTP `401`/`403` on the upgrade request.
    #[error(
        "hub refused the compiler credentials ({0}); check GX_API_KEY, GX_COMPILER_ID and \
         GX_COMPILER_SECRET"
    )]
    AuthRejected(String),
    /// A submission was answered `401` or `403`.
    #[error(
        "hub refused a submission with HTTP {0}; check GX_API_KEY (needs compile:submit), \
         GX_COMPILER_SECRET, and that the compiler is in the build's layer snapshot"
    )]
    Credentials(u16),
    /// A submission kept failing with a server error or a transport error.
    #[error("submission failed after {attempts} attempts: {last}")]
    Exhausted {
        /// Attempts made.
        attempts: u32,
        /// The last failure.
        last: String,
    },
    /// The socket could not be opened, or failed while open.
    #[error("websocket: {0}")]
    Socket(String),
    /// Nothing arrived on the socket for longer than the idle timeout.
    #[error("no frame from the hub for {0} s")]
    Idle(u64),
    /// [`HubClient::next_job`] was called with no open socket.
    #[error("not connected")]
    NotConnected,
    /// The HTTP client could not be built.
    #[error("http client: {0}")]
    Http(String),
}

/// Exponential backoff: `base` doubling to `max`, each delay drawn uniformly
/// from the upper half of its nominal value, `[nominal / 2, nominal]`, by a
/// seeded generator. The sequence of delays depends only on the policy and
/// the seed.
#[derive(Clone, Debug)]
pub struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
    rng: SplitMix64,
}

impl Backoff {
    /// The protocol's policy: 1 s doubling to 60 s.
    pub fn standard(seed: u64) -> Backoff {
        Backoff::new(Duration::from_secs(1), Duration::from_secs(60), seed)
    }

    /// A policy with the given first and largest nominal delay.
    pub fn new(base: Duration, max: Duration, seed: u64) -> Backoff {
        Backoff {
            base,
            max,
            attempt: 0,
            rng: SplitMix64(seed),
        }
    }

    /// Starts the doubling again from `base`. The generator is not reset, so
    /// later delays still follow the one seeded sequence.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// The delay before the next attempt.
    pub fn next_delay(&mut self) -> Duration {
        let factor = 1u128 << self.attempt.min(32);
        let nominal = (self.base.as_millis() * factor).min(self.max.as_millis()) as u64;
        self.attempt = self.attempt.saturating_add(1);
        let half = nominal / 2;
        Duration::from_millis(half + self.rng.next_u64() % (nominal - half + 1))
    }
}

/// The SplitMix64 generator: small, fast, and identical on every platform.
#[derive(Clone, Debug)]
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// FNV-1a of a string: a stable per-key seed for submission retries.
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Installs rustls' `ring` provider as the process default. Both the
/// WebSocket and the HTTP client use rustls; neither links OpenSSL.
fn install_crypto_provider() {
    // An error means a provider is already installed, which is fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Posts compiled sections to the hub. Cheap to clone; clones share one
/// connection pool.
#[derive(Clone, Debug)]
pub struct Submitter {
    config: Arc<HubConfig>,
    http: reqwest::Client,
    retry_base: Duration,
    max_attempts: u32,
    seed: u64,
}

impl Submitter {
    /// Submits `bytes` as this compiler's section for `chunk_key` of a build.
    ///
    /// - `201`: [`SubmitOutcome::Created`].
    /// - `409`: [`SubmitOutcome::AlreadyStored`].
    /// - `400`: [`SubmitOutcome::Rejected`] with the parsed `{code, name,
    ///   reason}` body when there is one. Never retried.
    /// - `401`, `403`: [`HubError::Credentials`].
    /// - `5xx` or a transport error: retried with [`Backoff`] up to the
    ///   attempt limit, then [`HubError::Exhausted`]. The jitter seed is the
    ///   submitter's seed mixed with the chunk key, so retries are reproducible.
    /// - Anything else: [`SubmitOutcome::Unexpected`].
    pub async fn submit(
        &self,
        space_id: &str,
        build_id: &str,
        chunk_key: &str,
        bytes: Vec<u8>,
    ) -> Result<Submission, HubError> {
        let url = self.config.submit_url(space_id, build_id, chunk_key);
        let mut backoff = Backoff::new(
            self.retry_base,
            Duration::from_secs(60),
            self.seed ^ fnv1a(chunk_key),
        );
        let body = bytes::Bytes::from(bytes);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let last = match self.post(url.clone(), body.clone()).await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let outcome = match status {
                        201 => Some(SubmitOutcome::Created),
                        409 => Some(SubmitOutcome::AlreadyStored {
                            reason: resp
                                .bytes()
                                .await
                                .ok()
                                .and_then(|b| serde_json::from_slice::<ConflictBody>(&b).ok())
                                .map(|c| c.reason),
                        }),
                        400 => Some(SubmitOutcome::Rejected(
                            resp.bytes()
                                .await
                                .ok()
                                .and_then(|b| serde_json::from_slice::<Rejection>(&b).ok()),
                        )),
                        401 | 403 => return Err(HubError::Credentials(status)),
                        500..=599 => None,
                        _ => Some(SubmitOutcome::Unexpected),
                    };
                    match outcome {
                        Some(outcome) => {
                            return Ok(Submission {
                                status,
                                outcome,
                                attempts: attempt,
                            })
                        }
                        None => format!("HTTP {status}"),
                    }
                }
                // Without the URL: reqwest errors carry no headers, but the
                // URL is not needed to understand the failure either.
                Err(e) => e.without_url().to_string(),
            };
            if attempt >= self.max_attempts {
                return Err(HubError::Exhausted {
                    attempts: attempt,
                    last,
                });
            }
            let delay = backoff.next_delay();
            warn!(
                chunk_key,
                attempt,
                error = %last,
                delay_ms = delay.as_millis() as u64,
                "submission failed, retrying"
            );
            tokio::time::sleep(delay).await;
        }
    }

    async fn post(&self, url: Url, body: bytes::Bytes) -> reqwest::Result<reqwest::Response> {
        self.http
            .post(url)
            .header("X-API-Key", self.config.api_key.expose())
            .header("X-Compiler-Secret", self.config.compiler_secret.expose())
            .header("X-Layer-Section-Type", SECTION_TYPE)
            .header("Content-Type", "application/octet-stream")
            .body(body)
            .send()
            .await
    }
}

/// The `{reason}` body of the hub's ambiguous-layer `409` (`Models/ConflictReason.cs`).
#[derive(Deserialize)]
struct ConflictBody {
    reason: String,
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A connection to the hub: one WebSocket for jobs, one HTTP client for
/// submissions.
pub struct HubClient {
    config: Arc<HubConfig>,
    submitter: Submitter,
    socket: Option<Socket>,
    idle_timeout: Duration,
}

impl fmt::Debug for HubClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HubClient")
            .field("config", &self.config)
            .field("connected", &self.socket.is_some())
            .finish()
    }
}

impl HubClient {
    /// A client for `config`, not yet connected, with
    /// [`DEFAULT_IDLE_TIMEOUT`], [`DEFAULT_SUBMIT_ATTEMPTS`], the standard
    /// 1 s retry base, and [`DEFAULT_BACKOFF_SEED`].
    pub fn new(config: HubConfig) -> Result<HubClient, HubError> {
        install_crypto_provider();
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| HubError::Http(e.to_string()))?;
        let config = Arc::new(config);
        Ok(HubClient {
            submitter: Submitter {
                config: Arc::clone(&config),
                http,
                retry_base: Duration::from_secs(1),
                max_attempts: DEFAULT_SUBMIT_ATTEMPTS,
                seed: DEFAULT_BACKOFF_SEED,
            },
            config,
            socket: None,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        })
    }

    /// Sets how long the socket may be silent before [`HubClient::next_job`]
    /// reports [`HubError::Idle`].
    pub fn with_idle_timeout(mut self, timeout: Duration) -> HubClient {
        self.idle_timeout = timeout;
        self
    }

    /// Sets the submission retry policy: first delay and attempt limit.
    pub fn with_submit_retry(mut self, base: Duration, max_attempts: u32) -> HubClient {
        self.submitter.retry_base = base;
        self.submitter.max_attempts = max_attempts.max(1);
        self
    }

    /// Sets the seed of the submission retry jitter.
    pub fn with_seed(mut self, seed: u64) -> HubClient {
        self.submitter.seed = seed;
        self
    }

    /// The configuration this client was built with.
    pub fn config(&self) -> &HubConfig {
        &self.config
    }

    /// A handle that submits sections independently of the socket.
    pub fn submitter(&self) -> Submitter {
        self.submitter.clone()
    }

    /// Opens the WebSocket to `GET /compiler/connect`, replacing any open
    /// one. An HTTP `401` or `403` answer to the upgrade is
    /// [`HubError::AuthRejected`]; a close with `4401` arrives later, through
    /// [`HubClient::next_job`].
    pub async fn connect(&mut self) -> Result<(), HubError> {
        self.socket = None;
        let mut request = self
            .config
            .connect_url()
            .as_str()
            .into_client_request()
            .map_err(|e| HubError::Socket(e.to_string()))?;
        let headers = request.headers_mut();
        for (name, value) in [
            ("X-API-Key", self.config.api_key.expose()),
            ("X-Compiler-Secret", self.config.compiler_secret.expose()),
            ("X-Compiler-Id", self.config.compiler_id.as_str()),
        ] {
            let mut value = HeaderValue::from_str(value).map_err(|_| {
                HubError::Socket(format!("{name} holds characters not allowed in a header"))
            })?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        match tokio_tungstenite::connect_async(request).await {
            Ok((socket, _)) => {
                self.socket = Some(socket);
                Ok(())
            }
            Err(tungstenite::Error::Http(resp)) if matches!(resp.status().as_u16(), 401 | 403) => {
                Err(HubError::AuthRejected(format!(
                    "HTTP {} on the WebSocket upgrade",
                    resp.status().as_u16()
                )))
            }
            Err(tungstenite::Error::Http(resp)) => Err(HubError::Socket(format!(
                "HTTP {} on the WebSocket upgrade",
                resp.status().as_u16()
            ))),
            Err(e) => Err(HubError::Socket(e.to_string())),
        }
    }

    /// Waits for the next job.
    ///
    /// - `Ok(Some(job))`: a dispatch frame arrived.
    /// - `Ok(None)`: the hub closed the socket normally, or it ended.
    /// - `Err(HubError::AuthRejected)`: the hub closed with `4401`.
    /// - `Err(HubError::Idle)`: nothing arrived within the idle timeout.
    /// - `Err(HubError::Socket)`: the socket failed.
    ///
    /// Pings are answered as part of reading. Text frames that are not a job
    /// are logged at warn level and skipped; binary frames are ignored.
    /// Cancel-safe: dropping the future loses no frame.
    pub async fn next_job(&mut self) -> Result<Option<Job>, HubError> {
        let idle = self.idle_timeout;
        let socket = self.socket.as_mut().ok_or(HubError::NotConnected)?;
        loop {
            let frame = match tokio::time::timeout(idle, socket.next()).await {
                Err(_) => {
                    self.socket = None;
                    return Err(HubError::Idle(idle.as_secs()));
                }
                Ok(None) => {
                    self.socket = None;
                    return Ok(None);
                }
                Ok(Some(Err(e))) => {
                    self.socket = None;
                    return Err(HubError::Socket(e.to_string()));
                }
                Ok(Some(Ok(frame))) => frame,
            };
            match frame {
                Message::Text(text) => match serde_json::from_str::<Job>(text.as_str()) {
                    Ok(job) => return Ok(Some(job)),
                    Err(e) => {
                        warn!(error = %e, bytes = text.len(), "ignoring a text frame that is not a job")
                    }
                },
                Message::Close(close) => {
                    let code = close.as_ref().map(|c| u16::from(c.code));
                    // Answer the close; the socket is finished either way.
                    let _ = socket.close(None).await;
                    self.socket = None;
                    return match code {
                        Some(CLOSE_UNAUTHORIZED) => Err(HubError::AuthRejected(format!(
                            "WebSocket close code {CLOSE_UNAUTHORIZED}"
                        ))),
                        _ => {
                            info!(code = code.unwrap_or(1005), "hub closed the socket");
                            Ok(None)
                        }
                    };
                }
                Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {
                    // A pong is owed for a ping; flushing sends it now rather
                    // than on the next read.
                    let _ = socket.flush().await;
                }
            }
        }
    }

    /// Submits a section; see [`Submitter::submit`].
    pub async fn submit(
        &self,
        space_id: &str,
        build_id: &str,
        chunk_key: &str,
        bytes: Vec<u8>,
    ) -> Result<Submission, HubError> {
        self.submitter
            .submit(space_id, build_id, chunk_key, bytes)
            .await
    }
}

/// How [`serve`] runs.
#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// Jobs compiled and submitted at once; at least 1.
    pub workers: usize,
    /// Return after the socket closes the first time, once every job
    /// received has finished.
    pub once: bool,
    /// Reconnect policy.
    pub reconnect: Backoff,
    /// How sections are compiled.
    pub compile: CompileOptions,
}

impl Default for ServeOptions {
    /// [`DEFAULT_WORKERS`], not once, [`Backoff::standard`] with
    /// [`DEFAULT_BACKOFF_SEED`], default compile options.
    fn default() -> ServeOptions {
        ServeOptions {
            workers: DEFAULT_WORKERS,
            once: false,
            reconnect: Backoff::standard(DEFAULT_BACKOFF_SEED),
            compile: CompileOptions::default(),
        }
    }
}

/// Counts of what [`serve`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServeStats {
    /// Successful socket connections.
    pub connections: u64,
    /// Job frames received.
    pub jobs: u64,
    /// Jobs ignored because a job with the same id was in flight.
    pub duplicates: u64,
    /// Submissions answered `201`.
    pub created: u64,
    /// Submissions answered `409`.
    pub already_stored: u64,
    /// Submissions answered `400`.
    pub rejected: u64,
    /// Jobs that could not be compiled, or whose submission failed in any
    /// other way.
    pub failed: u64,
}

/// What the workers share.
struct Shared {
    system: Arc<System>,
    compile: CompileOptions,
    submitter: Submitter,
    permits: Arc<Semaphore>,
    in_flight: Mutex<HashSet<String>>,
    stats: Mutex<ServeStats>,
    fatal: mpsc::UnboundedSender<HubError>,
}

impl Shared {
    fn count(&self, f: impl FnOnce(&mut ServeStats)) {
        f(&mut self.stats.lock().expect("stats lock"));
    }
}

/// Why the connection loop stopped.
enum Stop {
    Interrupted,
    Once,
    Fatal(HubError),
}

/// Runs the compiler daemon: connects, receives jobs, compiles each with
/// [`compile`] on a blocking thread, submits it, and reconnects with
/// `opts.reconnect` whenever the socket drops or cannot be opened.
///
/// Returns when `shutdown` completes (in-flight jobs are abandoned), when
/// `opts.once` is set and the socket has closed once (in-flight jobs are
/// finished first), or with an error when the hub refuses the credentials,
/// on the socket or on a submission. Every job outcome is logged as one
/// line with the job id, chunk key, byte length, status, and elapsed
/// milliseconds.
pub async fn serve(
    system: Arc<System>,
    mut client: HubClient,
    mut opts: ServeOptions,
    shutdown: impl Future<Output = ()>,
) -> Result<ServeStats, HubError> {
    let (fatal_tx, mut fatal_rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        system,
        compile: opts.compile,
        submitter: client.submitter(),
        permits: Arc::new(Semaphore::new(opts.workers.max(1))),
        in_flight: Mutex::new(HashSet::new()),
        stats: Mutex::new(ServeStats::default()),
        fatal: fatal_tx,
    });
    let mut tasks = JoinSet::new();
    tokio::pin!(shutdown);
    info!(
        url = %client.config().connect_url(),
        workers = opts.workers.max(1),
        once = opts.once,
        "compiler starting"
    );

    let stop = 'outer: loop {
        let connected = tokio::select! {
            r = client.connect() => r,
            () = &mut shutdown => break 'outer Stop::Interrupted,
            Some(e) = fatal_rx.recv() => break 'outer Stop::Fatal(e),
        };
        match connected {
            Ok(()) => {
                opts.reconnect.reset();
                shared.count(|s| s.connections += 1);
                info!("connected to hub");
                loop {
                    tokio::select! {
                        r = client.next_job() => match r {
                            Ok(Some(job)) => accept_job(&shared, &mut tasks, job),
                            Ok(None) => break,
                            Err(e @ HubError::AuthRejected(_)) => break 'outer Stop::Fatal(e),
                            Err(e) => {
                                warn!(error = %e, "socket dropped");
                                break;
                            }
                        },
                        () = &mut shutdown => break 'outer Stop::Interrupted,
                        Some(e) = fatal_rx.recv() => break 'outer Stop::Fatal(e),
                        Some(_) = tasks.join_next() => {}
                    }
                }
                if opts.once {
                    break 'outer Stop::Once;
                }
            }
            Err(e @ HubError::AuthRejected(_)) => break 'outer Stop::Fatal(e),
            Err(e) => warn!(error = %e, "connect failed"),
        }
        let delay = opts.reconnect.next_delay();
        info!(delay_ms = delay.as_millis() as u64, "reconnecting");
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = &mut shutdown => break 'outer Stop::Interrupted,
            Some(e) = fatal_rx.recv() => break 'outer Stop::Fatal(e),
        }
    };

    let result = match stop {
        Stop::Interrupted => {
            tasks.abort_all();
            info!(abandoned = tasks.len(), "interrupted");
            Ok(())
        }
        Stop::Fatal(e) => {
            tasks.abort_all();
            Err(e)
        }
        Stop::Once => {
            while tasks.join_next().await.is_some() {}
            fatal_rx.try_recv().map_or(Ok(()), Err)
        }
    };
    let stats = *shared.stats.lock().expect("stats lock");
    info!(
        connections = stats.connections,
        jobs = stats.jobs,
        duplicates = stats.duplicates,
        created = stats.created,
        already_stored = stats.already_stored,
        rejected = stats.rejected,
        failed = stats.failed,
        "compiler stopped"
    );
    result.map(|()| stats)
}

/// Starts a job unless one with the same id is in flight.
fn accept_job(shared: &Arc<Shared>, tasks: &mut JoinSet<()>, job: Job) {
    shared.count(|s| s.jobs += 1);
    let fresh = shared
        .in_flight
        .lock()
        .expect("in-flight lock")
        .insert(job.job_id.clone());
    if !fresh {
        shared.count(|s| s.duplicates += 1);
        info!(job_id = %job.job_id, chunk_key = %job.chunk_key, "duplicate job ignored");
        return;
    }
    let shared = Arc::clone(shared);
    tasks.spawn(async move {
        let _permit = Arc::clone(&shared.permits).acquire_owned().await;
        run_job(&shared, &job).await;
        shared
            .in_flight
            .lock()
            .expect("in-flight lock")
            .remove(&job.job_id);
    });
}

/// Compiles and submits one job, and logs its outcome as one line.
async fn run_job(shared: &Shared, job: &Job) {
    let start = Instant::now();
    let elapsed_ms = || start.elapsed().as_millis() as u64;
    let (job_id, chunk_key) = (job.job_id.as_str(), job.chunk_key.as_str());

    let key = match ChunkKey::from_str(chunk_key) {
        Ok(k) => k,
        Err(e) => {
            shared.count(|s| s.failed += 1);
            error!(job_id, chunk_key, bytes = 0, status = "none", elapsed_ms = elapsed_ms(), error = %e, "chunk key does not parse");
            return;
        }
    };
    let system = Arc::clone(&shared.system);
    let opts = shared.compile;
    let compiled = tokio::task::spawn_blocking(move || compile(&system, &key, &opts)).await;
    let bytes = match compiled {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => {
            shared.count(|s| s.failed += 1);
            error!(job_id, chunk_key, bytes = 0, status = "none", elapsed_ms = elapsed_ms(), error = %e, "compile failed");
            return;
        }
        Err(e) => {
            shared.count(|s| s.failed += 1);
            error!(job_id, chunk_key, bytes = 0, status = "none", elapsed_ms = elapsed_ms(), error = %e, "compile panicked");
            return;
        }
    };
    let len = bytes.len();
    match shared
        .submitter
        .submit(&job.space_id, &job.build_id, chunk_key, bytes)
        .await
    {
        Ok(sub) => {
            let status = sub.status;
            match sub.outcome {
                SubmitOutcome::Created => {
                    shared.count(|s| s.created += 1);
                    info!(
                        job_id,
                        chunk_key,
                        bytes = len,
                        status,
                        elapsed_ms = elapsed_ms(),
                        "section stored"
                    );
                }
                SubmitOutcome::AlreadyStored { reason: None } => {
                    shared.count(|s| s.already_stored += 1);
                    info!(
                        job_id,
                        chunk_key,
                        bytes = len,
                        status,
                        elapsed_ms = elapsed_ms(),
                        "section already stored"
                    );
                }
                SubmitOutcome::AlreadyStored {
                    reason: Some(reason),
                } => {
                    shared.count(|s| s.already_stored += 1);
                    warn!(
                        job_id,
                        chunk_key,
                        bytes = len,
                        status,
                        elapsed_ms = elapsed_ms(),
                        reason,
                        "hub answered conflict with a reason"
                    );
                }
                SubmitOutcome::Rejected(Some(r)) => {
                    shared.count(|s| s.rejected += 1);
                    error!(job_id, chunk_key, bytes = len, status, elapsed_ms = elapsed_ms(), code = r.code, name = %r.name, reason = %r.reason, "section rejected");
                }
                SubmitOutcome::Rejected(None) => {
                    shared.count(|s| s.rejected += 1);
                    error!(
                        job_id,
                        chunk_key,
                        bytes = len,
                        status,
                        elapsed_ms = elapsed_ms(),
                        "section rejected without a reason"
                    );
                }
                SubmitOutcome::Unexpected => {
                    shared.count(|s| s.failed += 1);
                    error!(
                        job_id,
                        chunk_key,
                        bytes = len,
                        status,
                        elapsed_ms = elapsed_ms(),
                        "unexpected status"
                    );
                }
            }
        }
        Err(e @ HubError::Credentials(status)) => {
            shared.count(|s| s.failed += 1);
            error!(
                job_id,
                chunk_key,
                bytes = len,
                status,
                elapsed_ms = elapsed_ms(),
                "submission refused, stopping"
            );
            let _ = shared.fatal.send(e);
        }
        Err(e) => {
            shared.count(|s| s.failed += 1);
            error!(job_id, chunk_key, bytes = len, status = "none", elapsed_ms = elapsed_ms(), error = %e, "submission failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn config() -> HubConfig {
        HubConfig::from_lookup(lookup(&[
            ("GX_HUB_URL", "https://hub.example/base/"),
            ("GX_API_KEY", "key-value"),
            ("GX_COMPILER_ID", "cid"),
            ("GX_COMPILER_SECRET", "secret-value"),
        ]))
        .unwrap()
    }

    #[test]
    fn urls() {
        let c = config();
        assert_eq!(
            c.connect_url().as_str(),
            "wss://hub.example/base/compiler/connect"
        );
        assert_eq!(
            c.submit_url("s", "b", "4-1-0-0-0").as_str(),
            "https://hub.example/base/space/s/build/b/chunk/4-1-0-0-0/compiled"
        );
        assert_eq!(
            c.submit_url("s", "b", "a/b c").as_str(),
            "https://hub.example/base/space/s/build/b/chunk/a%2Fb%20c/compiled"
        );
    }

    #[test]
    fn debug_redacts_credentials() {
        let text = format!("{:?}", config());
        assert!(!text.contains("key-value"));
        assert!(!text.contains("secret-value"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn fallback_names_and_missing() {
        let c = HubConfig::from_lookup(lookup(&[
            ("HUB_URL", "http://localhost:1"),
            ("COMPILER_API_KEY", "k"),
            ("COMPILER_ID", "i"),
            ("COMPILER_SECRET", "s"),
            ("GX_COMPILER_ID", "primary"),
        ]))
        .unwrap();
        assert_eq!(c.compiler_id, "primary");
        assert_eq!(
            c.connect_url().as_str(),
            "ws://localhost:1/compiler/connect"
        );
        let err = HubConfig::from_lookup(lookup(&[("GX_HUB_URL", "http://h")])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "environment variable GX_API_KEY is not set (fallback COMPILER_API_KEY is not set either)"
        );
        let err = HubConfig::from_lookup(lookup(&[
            ("GX_HUB_URL", "ftp://h"),
            ("GX_API_KEY", "k"),
            ("GX_COMPILER_ID", "i"),
            ("GX_COMPILER_SECRET", "s"),
        ]))
        .unwrap_err();
        assert!(matches!(err, ConfigError::BadUrl));
    }

    #[test]
    fn backoff_doubles_to_the_cap_with_bounded_jitter() {
        let mut b = Backoff::standard(7);
        let delays: Vec<u64> = (0..10).map(|_| b.next_delay().as_millis() as u64).collect();
        let nominal = [
            1000, 2000, 4000, 8000, 16000, 32000, 60000, 60000, 60000, 60000,
        ];
        for (d, n) in delays.iter().zip(nominal) {
            assert!(*d >= n / 2 && *d <= n, "{d} outside [{}, {n}]", n / 2);
        }
        let mut again = Backoff::standard(7);
        let repeat: Vec<u64> = (0..10)
            .map(|_| again.next_delay().as_millis() as u64)
            .collect();
        assert_eq!(delays, repeat, "same seed, same delays");
        b.reset();
        assert!(b.next_delay() <= Duration::from_secs(1));
    }

    #[test]
    fn splitmix_reference_values() {
        // First outputs of SplitMix64 seeded with 0, as published with the
        // generator's reference implementation.
        let mut r = SplitMix64(0);
        assert_eq!(r.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(r.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    }

    #[test]
    fn job_frame_shape() {
        let frame = r#"{"jobId":"j","spaceId":"s","buildId":"b","chunkKey":"registry","compilerId":"c","layerId":"l"}"#;
        let job: Job = serde_json::from_str(frame).unwrap();
        assert_eq!(job.chunk_key, "registry");
        assert_eq!(serde_json::to_string(&job).unwrap(), frame);
    }
}
