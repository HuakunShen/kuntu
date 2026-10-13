//! HTTP JSON daemon for the independent file-search Rust workspace.
//!
//! The daemon is intentionally framework-free and reuses the same core and
//! SQLite crates as the CLI. It is a service adapter, not the search API
//! boundary; future desktop, REST, or native hosts can reuse the same request
//! handlers or replace the transport.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use kuntu_core::{BackendError, MetadataIndex, SearchConfig, SearchQuery, SearchRoot};
use kuntu_index::{match_names, KuntuIndex, WriterService};
use kuntu_store::{Home, StoreMode};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct DaemonConfig {
  pub db_path: PathBuf,
  pub store_mode: StoreMode,
  pub addr: String,
  pub duration: Option<Duration>,
  pub max_requests: Option<usize>,
  pub allowed_roots: Vec<SearchRoot>,
  pub allow_non_loopback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonRunStats {
  pub addr: String,
  pub requests: usize,
  pub elapsed_ms: u128,
  pub reader_connections: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpRequest {
  method: String,
  path: String,
  body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpResponse {
  status: u16,
  body: String,
}

#[derive(Debug, Deserialize)]
struct RootsRequest {
  roots: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SearchRequest {
  query: String,
  roots: Vec<String>,
  limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct SearchResultDto {
  path: String,
  score: i32,
  provider: String,
  matches: Vec<String>,
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const IO_POLL: Duration = Duration::from_millis(50);
struct QueuedConnection {
  stream: TcpStream,
  deadline: Instant,
}
fn io_timeout(
  deadline: Instant,
  cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Duration, BackendError> {
  if cancelled.load(std::sync::atomic::Ordering::Relaxed) || Instant::now() >= deadline {
    return Err(BackendError::new("request cancelled or deadline elapsed"));
  }
  Ok(
    deadline
      .saturating_duration_since(Instant::now())
      .min(IO_POLL)
      .max(Duration::from_millis(1)),
  )
}

pub fn serve(config: DaemonConfig) -> Result<DaemonRunStats, BackendError> {
  validate_bind_addr(&config)?;
  let listener = TcpListener::bind(&config.addr).map_err(BackendError::from)?;
  listener.set_nonblocking(true).map_err(BackendError::from)?;
  let addr = listener
    .local_addr()
    .map_err(BackendError::from)?
    .to_string();
  let started = Instant::now();
  let server_deadline = config.duration.map(|d| started + d);
  let cancelled = std::sync::atomic::AtomicBool::new(false);
  let shared = config.store_mode == StoreMode::Shared;
  let home = if shared {
    Some(
      Home::open(
        config
          .db_path
          .parent()
          .ok_or_else(|| BackendError::new("Missing store home."))?,
      )
      .map_err(BackendError::from)?,
    )
  } else {
    None
  };
  let _service = home.clone().map(WriterService::start);
  let init = if let Some(h) = &home {
    KuntuIndex::open_shared(h.clone(), false, "daemon")
  } else {
    KuntuIndex::open(&config.db_path)
  }
  .map_err(|e| BackendError::new(e.to_string()))?;
  for root in &config.allowed_roots {
    init
      .register_root(root)
      .map_err(|e| BackendError::new(e.to_string()))?;
  }
  drop(init);
  let mut requests = 0;
  let reader_connections = std::sync::atomic::AtomicUsize::new(0);
  let (sender, receiver) = std::sync::mpsc::sync_channel::<QueuedConnection>(8);
  let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
  thread::scope(|scope| -> Result<(), BackendError> {
    for _ in 0..4 {
      let receiver = receiver.clone();
      let home = home.clone();
      let config = &config;
      let reader_connections = &reader_connections;
      let cancelled = &cancelled;
      scope.spawn(move || {
        let mut reader = None;
        let mut writer = None;
        loop {
          let stream = receiver.lock().expect("worker queue").recv();
          let Ok(stream) = stream else { break };
          let QueuedConnection {
            mut stream,
            deadline,
          } = stream;
          let request = match read_http_request_until(&mut stream, deadline, cancelled) {
            Ok(Some(r)) => r,
            _ => continue,
          };
          let readonly = matches!(
            (request.method.as_str(), request.path.as_str()),
            ("GET", _) | ("POST", "/search")
          );
          let slot = if readonly { &mut reader } else { &mut writer };
          if slot.is_none() {
            if readonly {
              reader_connections.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let opened = if let Some(h) = &home {
              KuntuIndex::open_shared(h.clone(), readonly, "daemon")
            } else if readonly {
              KuntuIndex::open_read_only(&config.db_path)
            } else {
              KuntuIndex::open(&config.db_path)
            };
            match opened {
              Ok(index) => *slot = Some(index),
              Err(e) => {
                let _ = write_http_response_until(
                  &mut stream,
                  &json_response(500, json!({"error":e.to_string()})),
                  deadline,
                  cancelled,
                );
                continue;
              }
            }
          }
          let response = handle_request(
            slot.as_mut().expect("opened worker connection"),
            request,
            &config.allowed_roots,
          );
          let _ = write_http_response_until(&mut stream, &response, deadline, cancelled);
        }
      });
    }
    let outcome = (|| {
      'accepts: loop {
        if server_deadline.is_some_and(|d| Instant::now() >= d) {
          cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
          break;
        }
        if config.max_requests.is_some_and(|m| requests >= m) {
          break;
        }
        match listener.accept() {
          Ok((stream, _)) => {
            // macOS inherits nonblocking from accept; normalize it so the
            // short socket timeouts bound each read/write poll on every Unix.
            stream.set_nonblocking(false).map_err(BackendError::from)?;
            let now = Instant::now();
            let deadline =
              server_deadline.map_or(now + REQUEST_TIMEOUT, |d| d.min(now + REQUEST_TIMEOUT));
            let mut queued = QueuedConnection { stream, deadline };
            requests += 1;
            loop {
              if server_deadline.is_some_and(|d| Instant::now() >= d) {
                cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                break 'accepts;
              }
              if Instant::now() >= queued.deadline {
                break;
              }
              match sender.try_send(queued) {
                Ok(()) => break,
                Err(std::sync::mpsc::TrySendError::Full(returned)) => queued = returned,
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                  return Err(BackendError::new("worker queue disconnected"))
                }
              }
              thread::sleep(Duration::from_millis(2));
            }
          }
          Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            thread::sleep(Duration::from_millis(2))
          }
          Err(e) => return Err(BackendError::from(e)),
        }
      }
      Ok(())
    })();
    if outcome.is_err() {
      cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    drop(sender);
    outcome
  })?;
  Ok(DaemonRunStats {
    addr,
    requests,
    elapsed_ms: started.elapsed().as_millis(),
    reader_connections: reader_connections.load(std::sync::atomic::Ordering::Relaxed),
  })
}

fn handle_request(
  index: &mut KuntuIndex,
  request: HttpRequest,
  allowed_roots: &[SearchRoot],
) -> HttpResponse {
  match (request.method.as_str(), request.path.as_str()) {
    ("GET", "/health") => json_response(200, json!({ "ok": true })),
    ("GET", "/status") => match index.status() {
      Ok(status) => json_response(
        200,
        json!({
            "roots": status.into_iter().map(|root| {
                json!({
                    "path": root.path.to_string_lossy(),
                    "entry_count": root.entry_count,
                    "generation": root.generation,
                    "dirty": root.dirty,
                    "status": index.root_state_name(&root.path).unwrap_or_else(|_|"dirty".into()),
                    "last_full_scan_at": root.last_full_scan_at,
                    "last_incremental_at": root.last_incremental_at
                })
            }).collect::<Vec<_>>()
        }),
      ),
      Err(err) => json_response(500, json!({ "error": err.to_string() })),
    },
    ("POST", "/search") => match parse_json::<SearchRequest>(&request.body) {
      Ok(search) => {
        let config = match config_from_requested_roots(&search.roots, allowed_roots) {
          Ok(config) => config,
          Err(err) => return json_response(400, json!({ "error": err.message })),
        };
        let mut query = SearchQuery::new(search.query);
        if let Some(limit) = search.limit {
          query.limit = limit;
        }
        match index.search_with_metrics(&config, &query) {
          Ok(outcome) => json_response(
            200,
            json!({
                "candidate_count": outcome.candidate_count,
                "truncated": outcome.truncated,
                "coverage": match outcome.coverage {
                    kuntu_index::CandidateCoverage::Complete => "complete",
                    kuntu_index::CandidateCoverage::Truncated => "truncated",
                    kuntu_index::CandidateCoverage::Building => "building",
                },
                "results": outcome.results.into_iter().map(SearchResultDto::from).collect::<Vec<_>>()
            }),
          ),
          Err(err) => json_response(500, json!({ "error": err.to_string() })),
        }
      }
      Err(err) => json_response(400, json!({ "error": err.message })),
    },
    ("POST", "/index/rebuild") => match parse_roots_request(&request.body, allowed_roots) {
      Ok(config) => {
        if index.is_shared() {
          return match index.enqueue("rebuild", &config) {
            Ok(id) => json_response(202, json!({"request_id":id})),
            Err(e) => json_response(409, json!({"error":e.to_string()})),
          };
        }
        match index.rebuild_index(&config) {
          Ok(stats) => json_response(
            200,
            json!({
                "stats": {
                    "roots": stats.roots,
                    "entries": stats.entries,
                    "skipped": stats.skipped,
                    "errors": stats.errors
                }
            }),
          ),
          Err(err) => json_response(500, json!({ "error": err.to_string() })),
        }
      }
      Err(err) => json_response(400, json!({ "error": err.message })),
    },
    ("POST", "/index/refresh") => match parse_roots_request(&request.body, allowed_roots) {
      Ok(config) => {
        if index.is_shared() {
          return match index.enqueue("refresh", &config) {
            Ok(id) => json_response(202, json!({"request_id":id})),
            Err(e) => json_response(409, json!({"error":e.to_string()})),
          };
        }
        match index.refresh_index(&config) {
          Ok(stats) => json_response(
            200,
            json!({
                "stats": {
                    "roots": stats.roots,
                    "inserted": stats.inserted,
                    "updated": stats.updated,
                    "deleted": stats.deleted,
                    "unchanged": stats.unchanged,
                    "skipped": stats.skipped,
                    "errors": stats.errors
                }
            }),
          ),
          Err(err) => json_response(500, json!({ "error": err.to_string() })),
        }
      }
      Err(err) => json_response(400, json!({ "error": err.message })),
    },
    _ => json_response(404, json!({ "error": "not found" })),
  }
}

#[cfg(test)]
fn read_http_request(stream: &mut TcpStream) -> Result<Option<HttpRequest>, BackendError> {
  read_http_request_until(
    stream,
    Instant::now() + REQUEST_TIMEOUT,
    &std::sync::atomic::AtomicBool::new(false),
  )
}
fn read_http_request_until(
  stream: &mut TcpStream,
  deadline: Instant,
  cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Option<HttpRequest>, BackendError> {
  let mut buffer = Vec::new();
  let mut chunk = [0_u8; 4096];
  loop {
    stream
      .set_read_timeout(Some(io_timeout(deadline, cancelled)?))
      .map_err(BackendError::from)?;
    match stream.read(&mut chunk) {
      Ok(0) => break,
      Ok(count) => {
        buffer.extend_from_slice(&chunk[..count]);
        if request_is_complete(&buffer) {
          break;
        }
      }
      Err(e)
        if matches!(
          e.kind(),
          std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
        ) =>
      {
        continue
      }
      Err(e) => return Err(BackendError::from(e)),
    }
  }
  if buffer.is_empty() {
    return Ok(None);
  }
  io_timeout(deadline, cancelled)?;
  parse_http_request(&buffer).map(Some)
}

fn parse_http_request(buffer: &[u8]) -> Result<HttpRequest, BackendError> {
  let header_end =
    find_header_end(buffer).ok_or_else(|| BackendError::new("request headers were incomplete"))?;
  let headers = String::from_utf8_lossy(&buffer[..header_end]);
  let mut lines = headers.lines();
  let request_line = lines
    .next()
    .ok_or_else(|| BackendError::new("missing request line"))?;
  let mut parts = request_line.split_whitespace();
  let method = parts
    .next()
    .ok_or_else(|| BackendError::new("missing method"))?
    .to_string();
  let path = parts
    .next()
    .ok_or_else(|| BackendError::new("missing path"))?
    .to_string();
  Ok(HttpRequest {
    method,
    path,
    body: buffer[header_end + 4..].to_vec(),
  })
}

fn request_is_complete(buffer: &[u8]) -> bool {
  let Some(header_end) = find_header_end(buffer) else {
    return false;
  };
  let content_length = content_length(&buffer[..header_end]).unwrap_or(0);
  buffer.len() >= header_end + 4 + content_length
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
  buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(headers: &[u8]) -> Option<usize> {
  let headers = String::from_utf8_lossy(headers);
  headers.lines().find_map(|line| {
    let (name, value) = line.split_once(':')?;
    if name.eq_ignore_ascii_case("content-length") {
      value.trim().parse::<usize>().ok()
    } else {
      None
    }
  })
}

#[cfg(test)]
fn write_http_response(
  stream: &mut TcpStream,
  response: &HttpResponse,
) -> Result<(), BackendError> {
  write_http_response_until(
    stream,
    response,
    Instant::now() + REQUEST_TIMEOUT,
    &std::sync::atomic::AtomicBool::new(false),
  )
}
fn write_http_response_until(
  stream: &mut TcpStream,
  response: &HttpResponse,
  deadline: Instant,
  cancelled: &std::sync::atomic::AtomicBool,
) -> Result<(), BackendError> {
  let reason = match response.status {
    200 => "OK",
    400 => "Bad Request",
    404 => "Not Found",
    500 => "Internal Server Error",
    _ => "OK",
  };
  let text = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        reason,
        response.body.len(),
        response.body
    );
  let mut remaining = text.as_bytes();
  while !remaining.is_empty() {
    stream
      .set_write_timeout(Some(io_timeout(deadline, cancelled)?))
      .map_err(BackendError::from)?;
    match stream.write(remaining) {
      Ok(0) => return Err(BackendError::new("response write returned zero")),
      Ok(n) => remaining = &remaining[n..],
      Err(e)
        if matches!(
          e.kind(),
          std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
        ) =>
      {
        continue
      }
      Err(e) => return Err(BackendError::from(e)),
    }
  }
  Ok(())
}

fn parse_roots_request(
  body: &[u8],
  allowed_roots: &[SearchRoot],
) -> Result<SearchConfig, BackendError> {
  let request = parse_json::<RootsRequest>(body)?;
  config_from_requested_roots(&request.roots, allowed_roots)
}

fn parse_json<T>(body: &[u8]) -> Result<T, BackendError>
where
  T: for<'de> Deserialize<'de>,
{
  serde_json::from_slice(body).map_err(|err| BackendError::new(err.to_string()))
}

fn config_from_requested_roots(
  roots: &[String],
  allowed_roots: &[SearchRoot],
) -> Result<SearchConfig, BackendError> {
  if roots.is_empty() {
    return Err(BackendError::new("at least one root is required"));
  }
  let allowed = allowed_roots
    .iter()
    .filter(|root| root.enabled)
    .collect::<Vec<_>>();
  if allowed.is_empty() {
    return Err(BackendError::new("daemon has no allowed roots"));
  }

  let mut requested_roots = Vec::with_capacity(roots.len());
  for root in roots {
    let requested = SearchRoot::new(root);
    let Some(allowed_root) = allowed
      .iter()
      .copied()
      .find(|allowed_root| requested.path.starts_with(&allowed_root.path))
    else {
      return Err(BackendError::new(format!(
        "root is outside daemon allowed roots: {}",
        requested.path.to_string_lossy()
      )));
    };
    requested_roots.push(
      requested
        .include_hidden(allowed_root.include_hidden)
        .include_ignored(allowed_root.include_ignored)
        .with_priority(allowed_root.priority),
    );
  }

  Ok(SearchConfig {
    roots: requested_roots,
  })
}

pub fn validate_bind_addr(config: &DaemonConfig) -> Result<(), BackendError> {
  if config.allow_non_loopback || addr_is_loopback(&config.addr) {
    return Ok(());
  }
  Err(BackendError::new(
    "non-loopback daemon bind requires --allow-non-loopback",
  ))
}

fn addr_is_loopback(addr: &str) -> bool {
  if let Ok(socket_addr) = addr.parse::<SocketAddr>() {
    return socket_addr.ip().is_loopback();
  }
  addr
    .split_once(':')
    .is_some_and(|(host, _port)| host == "localhost")
}

fn json_response(status: u16, value: serde_json::Value) -> HttpResponse {
  HttpResponse {
    status,
    body: value.to_string(),
  }
}

impl From<kuntu_core::SearchResult> for SearchResultDto {
  fn from(result: kuntu_core::SearchResult) -> Self {
    Self {
      path: result.path.to_string_lossy().into_owned(),
      score: result.score,
      provider: result.provider,
      matches: match_names(&result.matches)
        .into_iter()
        .map(ToOwned::to_owned)
        .collect(),
    }
  }
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::time::{SystemTime, UNIX_EPOCH};

  use super::*;

  #[test]
  fn parse_http_request_reads_method_path_and_body() {
    let request =
      parse_http_request(b"POST /search HTTP/1.1\r\nContent-Length: 13\r\n\r\n{\"query\":\"x\"}")
        .unwrap();

    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/search");
    assert_eq!(request.body, br#"{"query":"x"}"#);
  }

  #[test]
  fn health_endpoint_returns_ok_json() {
    let mut index = KuntuIndex::open_memory().unwrap();

    let response = handle_request(
      &mut index,
      HttpRequest {
        method: "GET".to_string(),
        path: "/health".to_string(),
        body: Vec::new(),
      },
      &[],
    );

    assert_eq!(response.status, 200);
    assert!(response.body.contains("\"ok\":true"));
  }

  #[test]
  fn search_endpoint_returns_indexed_results() {
    let root = temp_dir("search");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("daemon-target.md"), "daemon\n").unwrap();
    let mut index = KuntuIndex::open_memory().unwrap();
    let allowed_roots = vec![SearchRoot::new(&root)];
    // Paths are serialized, not format!-ed: a Windows path's backslashes are
    // JSON escapes, and a hand-built literal is invalid JSON off POSIX.
    let rebuild = handle_request(
      &mut index,
      HttpRequest {
        method: "POST".to_string(),
        path: "/index/rebuild".to_string(),
        body: serde_json::to_vec(&json!({ "roots": [root.to_string_lossy()] })).unwrap(),
      },
      &allowed_roots,
    );
    let response = handle_request(
      &mut index,
      HttpRequest {
        method: "POST".to_string(),
        path: "/search".to_string(),
        body: serde_json::to_vec(&json!({
          "query": "daemon target",
          "roots": [root.to_string_lossy()],
          "limit": 5
        }))
        .unwrap(),
      },
      &allowed_roots,
    );
    let _ = fs::remove_dir_all(&root);

    assert_eq!(rebuild.status, 200);
    assert_eq!(response.status, 200);
    assert!(response.body.contains("daemon-target.md"));
    assert!(response.body.contains("candidate_count"));
  }

  #[test]
  fn rejects_roots_outside_daemon_allowlist() {
    let allowed = temp_dir("allowed");
    let denied = temp_dir("denied");
    fs::create_dir_all(&allowed).unwrap();
    fs::create_dir_all(&denied).unwrap();
    let allowed_roots = vec![SearchRoot::new(&allowed)];

    let err = config_from_requested_roots(&[denied.to_string_lossy().into_owned()], &allowed_roots)
      .unwrap_err();

    let _ = fs::remove_dir_all(&allowed);
    let _ = fs::remove_dir_all(&denied);

    assert!(err.message.contains("outside daemon allowed roots"));
  }

  #[test]
  fn daemon_rejects_non_loopback_bind_without_opt_in() {
    let config = DaemonConfig {
      store_mode: StoreMode::Private,
      db_path: PathBuf::from("/tmp/kfs-review-daemon.sqlite"),
      addr: "0.0.0.0:0".to_string(),
      duration: Some(Duration::from_millis(1)),
      max_requests: Some(0),
      allowed_roots: Vec::new(),
      allow_non_loopback: false,
    };

    let err = validate_bind_addr(&config).unwrap_err();

    assert!(err.message.contains("non-loopback"));
  }

  fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    std::env::temp_dir().join(format!("kuntu-daemon-{name}-{nonce}"))
  }
  #[test]
  fn non_holder_bulk_endpoint_queues_and_empty_search_is_rejected() {
    let base = PathBuf::from(std::env::var_os("KUNTU_TEST_DIR").expect("scratch KUNTU_TEST_DIR"));
    let home = Home::open(&base.join(format!("daemon-shared-{}", std::process::id()))).unwrap();
    let root = home.path.join("files");
    std::fs::create_dir_all(&root).unwrap();
    let roots = vec![SearchRoot::new(&root)];
    let mut holder = KuntuIndex::open_shared(home.clone(), false, "holder").unwrap();
    assert!(holder.acquire_writer().unwrap());
    let mut client = KuntuIndex::open_shared(home, false, "daemon").unwrap();
    client.register_root(&roots[0]).unwrap();
    let response = handle_request(
      &mut client,
      HttpRequest {
        method: "POST".into(),
        path: "/index/rebuild".into(),
        body: serde_json::to_vec(&json!({"roots":[root.to_string_lossy()]})).unwrap(),
      },
      &roots,
    );
    assert_eq!(response.status, 202);
    let response = handle_request(
      &mut client,
      HttpRequest {
        method: "POST".into(),
        path: "/search".into(),
        body: br#"{"roots":[],"query":""}"#.to_vec(),
      },
      &roots,
    );
    assert_eq!(response.status, 400);
  }
  #[test]
  fn strict_daemon_admission_ignores_another_consumers_union() {
    let base = PathBuf::from(std::env::var_os("KUNTU_TEST_DIR").expect("scratch directory"));
    let path = base.join(format!("daemon-policy-{}", std::process::id()));
    let home = Home::open(&path).unwrap();
    let root = home.path.join("files");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("visible-file.md"), "x").unwrap();
    std::fs::write(root.join(".hidden-file.md"), "x").unwrap();
    let allowed = SearchRoot::new(&root);
    let mut permissive = KuntuIndex::open_shared(home.clone(), false, "permissive").unwrap();
    let mut broad = allowed.clone();
    broad.include_hidden = true;
    permissive.register_root(&broad).unwrap();
    assert!(permissive.acquire_writer().unwrap());
    permissive
      .rebuild(&SearchConfig { roots: vec![broad] })
      .unwrap();
    let mut daemon = KuntuIndex::open_shared(home, false, "daemon").unwrap();
    daemon.register_root(&allowed).unwrap();
    let response = handle_request(
      &mut daemon,
      HttpRequest {
        method: "POST".into(),
        path: "/search".into(),
        body: serde_json::to_vec(
          &json!({"query":"file","roots":[root.to_string_lossy()],"include_hidden":true}),
        )
        .unwrap(),
      },
      &[allowed],
    );
    assert_eq!(response.status, 200);
    assert!(response.body.contains("visible-file"));
    assert!(!response.body.contains(".hidden-file"));
  }
}

#[cfg(test)]
mod connection_tests {
  use super::*;
  fn health_run(db_path: PathBuf, requests: usize, mode: StoreMode) -> (DaemonRunStats, Vec<f64>) {
    let addr = TcpListener::bind("127.0.0.1:0")
      .unwrap()
      .local_addr()
      .unwrap()
      .to_string();
    let config = DaemonConfig {
      store_mode: mode,
      db_path,
      addr: addr.clone(),
      duration: None,
      max_requests: Some(requests),
      allowed_roots: vec![],
      allow_non_loopback: false,
    };
    let worker = thread::spawn(move || serve(config).unwrap());
    let mut samples = Vec::new();
    for _ in 0..requests {
      let deadline = Instant::now() + Duration::from_secs(10);
      let start = Instant::now();
      let mut stream = loop {
        match TcpStream::connect(&addr) {
          Ok(stream) => break stream,
          Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(1)),
          Err(e) => panic!("daemon startup: {e}"),
        }
      };
      stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
      stream.write_all(b"GET /health HTTP/1.1\r\n\r\n").unwrap();
      let mut response = String::new();
      stream.read_to_string(&mut response).unwrap();
      assert!(response.contains("\"ok\":true"), "{response}");
      samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    (worker.join().unwrap(), samples)
  }
  #[test]
  fn health_requests_reuse_worker_reader_connections() {
    let path = std::env::temp_dir().join(format!("daemon-pool-{}.sqlite", std::process::id()));
    let (stats, _) = health_run(path, 16, StoreMode::Private);
    assert!(
      stats.reader_connections <= 4,
      "opened {} readers for 16 requests",
      stats.reader_connections
    );
  }
  #[test]
  #[ignore = "owner-supplied 1M shared-store copy; release latency evidence"]
  fn million_store_daemon_health_latency() {
    let path = PathBuf::from(std::env::var_os("KUNTU_HEALTH_DB").unwrap());
    let (stats, samples) = health_run(path, 20, StoreMode::Shared);
    eprintln!(
      "M2_HEALTH reader_connections={} first_ms={:.3} warm_ms={:?}",
      stats.reader_connections,
      samples[0],
      &samples[1..]
    );
  }
}

#[cfg(test)]
mod deadline_tests {
  use super::*;
  use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
  };
  fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    (listener.accept().unwrap().0, client)
  }
  #[test]
  fn trickled_input_cannot_extend_the_absolute_request_deadline() {
    let (mut server, mut client) = pair();
    let trickler = thread::spawn(move || {
      let end = Instant::now() + Duration::from_secs(3);
      while Instant::now() < end {
        if client.write_all(b"x").is_err() {
          break;
        }
        thread::sleep(Duration::from_millis(30));
      }
    });
    let start = Instant::now();
    assert!(read_http_request(&mut server).is_err());
    let elapsed = start.elapsed();
    drop(server);
    trickler.join().unwrap();
    assert!(
      elapsed < Duration::from_millis(2500),
      "trickled read took {elapsed:?}"
    );
  }
  #[test]
  fn stalled_response_writes_have_an_absolute_deadline() {
    let (mut server, client) = pair();
    let blocked = thread::spawn(move || {
      thread::sleep(Duration::from_secs(3));
      drop(client);
    });
    let response = HttpResponse {
      status: 200,
      body: "x".repeat(64 * 1024 * 1024),
    };
    let start = Instant::now();
    assert!(write_http_response(&mut server, &response).is_err());
    let elapsed = start.elapsed();
    drop(server);
    blocked.join().unwrap();
    assert!(
      elapsed < Duration::from_millis(2500),
      "stalled write took {elapsed:?}"
    );
  }
  #[test]
  fn shutdown_cancels_busy_workers_and_full_queue_submission() {
    let addr = TcpListener::bind("127.0.0.1:0")
      .unwrap()
      .local_addr()
      .unwrap()
      .to_string();
    let config = DaemonConfig {
      store_mode: StoreMode::Private,
      db_path: std::env::temp_dir().join(format!("daemon-shutdown-{}.sqlite", std::process::id())),
      addr: addr.clone(),
      duration: Some(Duration::from_millis(250)),
      max_requests: None,
      allowed_roots: vec![],
      allow_non_loopback: false,
    };
    let (done, result) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
      done.send(serve(config)).unwrap();
    });
    let start = Instant::now();
    let mut clients = Vec::new();
    for _ in 0..14 {
      let stream = loop {
        match TcpStream::connect(&addr) {
          Ok(stream) => break stream,
          Err(_) if start.elapsed() < Duration::from_secs(5) => {
            thread::sleep(Duration::from_millis(1))
          }
          Err(e) => panic!("connect: {e}"),
        }
      };
      let mut stream = stream;
      stream.write_all(b"x").unwrap();
      clients.push(stream);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let trickle_stop = stop.clone();
    let trickler = thread::spawn(move || {
      while !trickle_stop.load(Ordering::Relaxed) {
        for stream in &mut clients {
          let _ = stream.write_all(b"x");
        }
        thread::sleep(Duration::from_millis(20));
      }
    });
    let timely = result.recv_timeout(Duration::from_secs(1));
    stop.store(true, Ordering::Relaxed);
    trickler.join().unwrap();
    server.join().unwrap();
    let stats = timely
      .expect("shutdown blocked behind occupied workers/full queue")
      .unwrap();
    eprintln!("SHUTDOWN stats={stats:?}");
    assert!(
      stats.requests >= 12,
      "did not fill four workers and eight queue slots: {stats:?}"
    );
    assert!(stats.elapsed_ms < 1000, "{stats:?}");
  }
}
