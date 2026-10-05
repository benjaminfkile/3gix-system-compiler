//! The hub client and the `serve` daemon against a mock hub.
//!
//! The mock speaks the hub's compiler protocol as written in
//! `docs/protocol.md`: a WebSocket at `/compiler/connect` (axum's WebSocket
//! support, which is `tokio-tungstenite` on the server side) that checks the
//! connect headers, pushes dispatch frames, and closes; and the submit
//! endpoint, which records every request and answers from a script.
//!
//! The binary is run as a child process where the exit code and the log
//! output matter; the library's [`serve`] is run in process where the
//! counts and timing matter.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use gx_core::key::ChunkKey;
use serde_json::{json, Value};
use system_compiler::hub::{
    self, Backoff, HubClient, HubConfig, HubError, ServeOptions, ServeStats,
};
use system_compiler::{compile, CompileOptions};

const API_KEY: &str = "mock-api-key-7f3a";
const SECRET: &str = "mock-compiler-secret-91c2";
const COMPILER_ID: &str = "c0000000-0000-0000-0000-000000000001";
const SPACE_ID: &str = "a0000000-0000-0000-0000-000000000001";
const BUILD_ID: &str = "b0000000-0000-0000-0000-000000000001";
const LAYER_ID: &str = "matter";

/// The registry, a cell of the Earth's frame that holds matter, and a cell
/// of the root frame, which is empty.
const KEYS: [&str; 3] = ["registry", "4-1-0-0-0", "0-0-0-0-0"];

/// What the mock does with one WebSocket connection.
#[derive(Clone)]
enum Conn {
    /// Push these frames, then close normally or stay open.
    Push(Vec<Value>, bool),
    /// Close with 4401 straight after the upgrade.
    Reject,
}

/// How the mock answers a submission.
#[derive(Clone, Copy)]
enum Answer {
    Created,
    Conflict,
    Invalid,
    Unauthorized,
}

struct Plan {
    conns: Vec<Conn>,
    answers: Vec<Answer>,
    answer_delay: Duration,
}

struct Submitted {
    space: String,
    build: String,
    key: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

#[derive(Default)]
struct Record {
    connects: Vec<(Instant, HeaderMap)>,
    closes: Vec<Instant>,
    submissions: Vec<Submitted>,
}

#[derive(Clone)]
struct Mock {
    plan: Arc<Plan>,
    record: Arc<Mutex<Record>>,
}

impl Mock {
    fn record(&self) -> std::sync::MutexGuard<'_, Record> {
        self.record.lock().unwrap()
    }
}

fn job(id: &str, key: &str) -> Value {
    json!({
        "jobId": id,
        "spaceId": SPACE_ID,
        "buildId": BUILD_ID,
        "chunkKey": key,
        "compilerId": COMPILER_ID,
        "layerId": LAYER_ID,
    })
}

fn three_jobs() -> Vec<Value> {
    KEYS.iter()
        .enumerate()
        .map(|(i, k)| job(&format!("d0000000-0000-0000-0000-00000000000{i}"), k))
        .collect()
}

fn header<'a>(h: &'a HeaderMap, name: &str) -> &'a str {
    h.get(name).and_then(|v| v.to_str().ok()).unwrap_or("")
}

async fn connect(State(m): State<Mock>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    // The hub's API key middleware answers before the upgrade.
    if header(&headers, "X-API-Key") != API_KEY {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let index = {
        let mut r = m.record();
        r.connects.push((Instant::now(), headers.clone()));
        r.connects.len() - 1
    };
    let creds_ok = header(&headers, "X-Compiler-Secret") == SECRET
        && header(&headers, "X-Compiler-Id") == COMPILER_ID;
    let conn = match m.plan.conns.get(index) {
        _ if !creds_ok => Conn::Reject,
        Some(c) => c.clone(),
        None => Conn::Push(Vec::new(), true),
    };
    ws.on_upgrade(move |socket| run_conn(m, conn, socket))
}

async fn run_conn(m: Mock, conn: Conn, mut socket: WebSocket) {
    let (frames, stay_open) = match conn {
        Conn::Reject => {
            let close = CloseFrame {
                code: 4401,
                reason: "Unauthorized".into(),
            };
            let _ = socket.send(Message::Close(Some(close))).await;
            while let Some(Ok(_)) = socket.recv().await {}
            return;
        }
        Conn::Push(frames, stay_open) => (frames, stay_open),
    };
    for f in frames {
        if socket
            .send(Message::Text(f.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }
    if !stay_open {
        m.record().closes.push(Instant::now());
        let _ = socket.send(Message::Close(None)).await;
    }
    while let Some(Ok(_)) = socket.recv().await {}
}

async fn submit(
    State(m): State<Mock>,
    Path((space, build, key)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let index = {
        let mut r = m.record();
        r.submissions.push(Submitted {
            space,
            build,
            key,
            headers,
            body: body.to_vec(),
        });
        r.submissions.len() - 1
    };
    tokio::time::sleep(m.plan.answer_delay).await;
    match m
        .plan
        .answers
        .get(index)
        .copied()
        .unwrap_or(Answer::Created)
    {
        Answer::Created => StatusCode::CREATED.into_response(),
        Answer::Conflict => StatusCode::CONFLICT.into_response(),
        Answer::Invalid => (
            StatusCode::BAD_REQUEST,
            Json(json!({"code": 102, "name": "BAD_MAGIC", "reason": "mock rejection"})),
        )
            .into_response(),
        Answer::Unauthorized => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// Starts a mock hub on a free local port; returns its base URL.
async fn start(plan: Plan) -> (String, Mock) {
    let mock = Mock {
        plan: Arc::new(plan),
        record: Arc::default(),
    };
    let app = Router::new()
        .route("/compiler/connect", get(connect))
        .route(
            "/space/{space}/build/{build}/chunk/{key}/compiled",
            post(submit),
        )
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, mock)
}

fn config(url: &str, api_key: &str) -> HubConfig {
    HubConfig::from_lookup(|name| {
        match name {
            "GX_HUB_URL" => Some(url),
            "GX_API_KEY" => Some(api_key),
            "GX_COMPILER_ID" => Some(COMPILER_ID),
            "GX_COMPILER_SECRET" => Some(SECRET),
            _ => None,
        }
        .map(str::to_string)
    })
    .unwrap()
}

/// Runs `system-compiler serve` against `url`; returns success and stderr.
async fn run_binary(url: &str, args: &[&str]) -> (bool, String) {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/system.toml");
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_system-compiler"));
    cmd.arg("serve").arg("--data").arg(data).args(args);
    for (primary, fallback) in hub::ENV_VARS {
        cmd.env_remove(primary).env_remove(fallback);
    }
    cmd.env("GX_HUB_URL", url)
        .env("GX_API_KEY", API_KEY)
        .env("GX_COMPILER_ID", COMPILER_ID)
        .env("GX_COMPILER_SECRET", SECRET)
        .env("RUST_LOG", "info")
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(60), cmd.output())
        .await
        .expect("serve finishes within 60 s")
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    eprintln!("{stderr}");
    (out.status.success(), stderr)
}

fn assert_no_credentials(text: &str) {
    assert!(!text.contains(SECRET), "the compiler secret leaked");
    assert!(!text.contains(API_KEY), "the API key leaked");
}

fn expected_bytes(key: &str) -> Vec<u8> {
    let key: ChunkKey = key.parse().unwrap();
    compile(&common::system(), &key, &CompileOptions::default()).unwrap()
}

fn basic_plan() -> Plan {
    Plan {
        conns: vec![Conn::Push(three_jobs(), false)],
        answers: vec![Answer::Created, Answer::Conflict, Answer::Invalid],
        answer_delay: Duration::ZERO,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_once_submits_three_sections() {
    let (url, mock) = start(basic_plan()).await;
    let (ok, stderr) = run_binary(&url, &["--once", "--workers", "4"]).await;
    assert!(ok, "serve --once exits cleanly");

    let r = mock.record();
    assert_eq!(r.connects.len(), 1);
    let h = &r.connects[0].1;
    assert_eq!(header(h, "X-API-Key"), API_KEY);
    assert_eq!(header(h, "X-Compiler-Secret"), SECRET);
    assert_eq!(header(h, "X-Compiler-Id"), COMPILER_ID);

    assert_eq!(r.submissions.len(), 3);
    let mut keys: Vec<&str> = r.submissions.iter().map(|s| s.key.as_str()).collect();
    keys.sort_unstable();
    let mut want = KEYS.to_vec();
    want.sort_unstable();
    assert_eq!(keys, want);
    for s in &r.submissions {
        assert_eq!(s.space, SPACE_ID);
        assert_eq!(s.build, BUILD_ID);
        assert_eq!(header(&s.headers, "X-API-Key"), API_KEY);
        assert_eq!(header(&s.headers, "X-Compiler-Secret"), SECRET);
        assert_eq!(header(&s.headers, "X-Layer-Section-Type"), "matter");
        assert_eq!(
            header(&s.headers, "Content-Type"),
            "application/octet-stream"
        );
        assert_eq!(s.body, expected_bytes(&s.key), "bytes for {}", s.key);
    }
    let registry = r.submissions.iter().find(|s| s.key == "registry").unwrap();
    assert_eq!(registry.body, expected_bytes("registry"));

    // One outcome line per job, with every field; the 400 at error level.
    let outcomes: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("job_id=") && l.contains("elapsed_ms="))
        .collect();
    assert_eq!(outcomes.len(), 3, "{outcomes:#?}");
    for line in &outcomes {
        for field in ["chunk_key=", "bytes=", "status="] {
            assert!(line.contains(field), "{field} missing in {line}");
        }
    }
    let rejected: Vec<&&str> = outcomes
        .iter()
        .filter(|l| l.contains("status=400"))
        .collect();
    assert_eq!(rejected.len(), 1);
    assert!(rejected[0].contains("ERROR"));
    assert!(rejected[0].contains("BAD_MAGIC"));
    assert!(rejected[0].contains("mock rejection"));
    assert!(stderr.contains("rejected=1"), "the rejection is counted");
    assert_no_credentials(&stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_counts_each_outcome() {
    let (url, _mock) = start(basic_plan()).await;
    let client = HubClient::new(config(&url, API_KEY)).unwrap();
    let opts = ServeOptions {
        once: true,
        ..ServeOptions::default()
    };
    let stats = hub::serve(
        Arc::new(common::system()),
        client,
        opts,
        std::future::pending(),
    )
    .await
    .unwrap();
    assert_eq!(
        stats,
        ServeStats {
            connections: 1,
            jobs: 3,
            duplicates: 0,
            created: 1,
            already_stored: 1,
            rejected: 1,
            failed: 0,
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnects_within_the_first_backoff_step() {
    let jobs = three_jobs();
    let (url, mock) = start(Plan {
        conns: vec![
            Conn::Push(jobs[..1].to_vec(), false),
            Conn::Push(jobs[1..].to_vec(), true),
        ],
        answers: Vec::new(),
        answer_delay: Duration::ZERO,
    })
    .await;
    let client = HubClient::new(config(&url, API_KEY)).unwrap();
    let opts = ServeOptions {
        reconnect: Backoff::standard(0),
        ..ServeOptions::default()
    };
    let watch = mock.clone();
    let shutdown = async move {
        while watch.record().submissions.len() < 3 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let stats = tokio::time::timeout(
        Duration::from_secs(30),
        hub::serve(Arc::new(common::system()), client, opts, shutdown),
    )
    .await
    .expect("all three jobs arrive")
    .unwrap();

    assert_eq!(stats.connections, 2);
    assert_eq!(stats.jobs, 3);
    let r = mock.record();
    assert_eq!(r.connects.len(), 2);
    let gap = r.connects[1].0 - r.closes[0];
    // The seeded first step, in [0.5 s, 1 s]; the second step would be at
    // least 1 s. The slack covers noticing the close and opening the socket.
    let first = Backoff::standard(0).next_delay();
    assert!(gap >= first, "reconnected after {gap:?}, before {first:?}");
    assert!(
        gap < first + Duration::from_millis(400),
        "reconnected after {gap:?}, first step {first:?}"
    );
    let mut keys: Vec<&str> = r.submissions.iter().map(|s| s.key.as_str()).collect();
    keys.sort_unstable();
    let mut want = KEYS.to_vec();
    want.sort_unstable();
    assert_eq!(keys, want);
}

#[tokio::test(flavor = "multi_thread")]
async fn close_4401_exits_non_zero_without_retrying() {
    let (url, mock) = start(Plan {
        conns: vec![Conn::Reject],
        answers: Vec::new(),
        answer_delay: Duration::ZERO,
    })
    .await;
    let (ok, stderr) = run_binary(&url, &[]).await;
    assert!(!ok, "a 4401 close is fatal");
    assert!(
        stderr.contains("refused the compiler credentials"),
        "{stderr}"
    );
    assert!(stderr.contains("4401"));
    assert_eq!(
        mock.record().connects.len(),
        1,
        "no retry with the same credentials"
    );
    assert_no_credentials(&stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn upgrade_refused_with_401_is_an_auth_failure() {
    let (url, mock) = start(basic_plan()).await;
    let client = HubClient::new(config(&url, "wrong-key")).unwrap();
    let err = hub::serve(
        Arc::new(common::system()),
        client,
        ServeOptions::default(),
        std::future::pending(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, HubError::AuthRejected(_)), "{err}");
    assert!(err.to_string().contains("HTTP 401"));
    assert!(mock.record().connects.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn submission_401_stops_the_daemon() {
    let jobs = three_jobs();
    let (url, mock) = start(Plan {
        conns: vec![Conn::Push(jobs[..1].to_vec(), true)],
        answers: vec![Answer::Unauthorized],
        answer_delay: Duration::ZERO,
    })
    .await;
    let (ok, stderr) = run_binary(&url, &[]).await;
    assert!(!ok, "a 401 on submit is fatal even with the socket open");
    assert!(stderr.contains("HTTP 401"), "{stderr}");
    assert_eq!(mock.record().submissions.len(), 1, "never retried");
    assert_no_credentials(&stderr);
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_job_in_flight_is_ignored() {
    let first = job("e0000000-0000-0000-0000-000000000001", "registry");
    let (url, mock) = start(Plan {
        conns: vec![Conn::Push(vec![first.clone(), first], false)],
        answers: Vec::new(),
        // Holds the first submission open while the duplicate arrives.
        answer_delay: Duration::from_millis(500),
    })
    .await;
    let client = HubClient::new(config(&url, API_KEY)).unwrap();
    let opts = ServeOptions {
        once: true,
        workers: 1,
        ..ServeOptions::default()
    };
    let stats = hub::serve(
        Arc::new(common::system()),
        client,
        opts,
        std::future::pending(),
    )
    .await
    .unwrap();
    assert_eq!(stats.jobs, 2);
    assert_eq!(stats.duplicates, 1);
    assert_eq!(stats.created, 1);
    assert_eq!(mock.record().submissions.len(), 1);
}
