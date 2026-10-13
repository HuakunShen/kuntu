//! Command-line adapter for the independent Rust file search prototype.
//!
//! The CLI exists for local testing and future integration experiments. It
//! requires explicit roots and delegates filtering/ranking to `kuntu-core`.

mod args;
mod output;

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use args::{parse_args, CommandSpec, IndexCommand, SearchProvider};
use kuntu_core::{
  CandidateProvider, MetadataIndex, SearchConfig, SearchEngineCore, SearchQuery, SearchResult,
};
use kuntu_daemon::{serve, DaemonConfig};
use kuntu_index::KuntuIndex;
use kuntu_provider_spotlight::SpotlightProvider;
use kuntu_store::{Home, StoreMode};
use kuntu_watcher::{run_polling_watch, WatchOptions};
use output::{
  format_bench_stats, format_daemon_stats, format_explain_text, format_rebuild_stats,
  format_refresh_stats, format_repair_stats, format_results_json, format_results_text,
  format_status, format_watch_stats,
};

#[derive(Debug, Clone)]
struct SearchRun {
  results: Vec<SearchResult>,
  candidate_count: usize,
}

fn main() -> ExitCode {
  match run(std::env::args().skip(1).collect()) {
    Ok(text) => {
      println!("{text}");
      ExitCode::SUCCESS
    }
    Err(err) => {
      eprintln!("kfs: {err}");
      ExitCode::from(2)
    }
  }
}

fn run(raw_args: Vec<String>) -> Result<String, String> {
  let spec = parse_args(&raw_args)?;
  let store_mode = spec.store_mode;
  let default_db = if spec.db_path.is_none() {
    Some(
      kuntu_store::resolve()
        .map_err(|e| e.to_string())?
        .join("index.v3.db"),
    )
  } else {
    None
  };
  let db_path = spec.db_path.as_deref().or(default_db.as_deref());
  let config = SearchConfig { roots: spec.roots };
  let core = SearchEngineCore::new(config.clone());

  match spec.command {
    CommandSpec::Store { operation, source } => {
      let home = Home::open(&kuntu_store::resolve().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
      match operation.as_str() {
        "info" => Ok(format!(
          "store_id={} epoch=3",
          home.id().map_err(|e| e.to_string())?
        )),
        "gc" => {
          let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
          let mut count = 0;
          for epoch in 1..3 {
            if home.gc_epoch(epoch, now).map_err(|e| e.to_string())? {
              count += 1;
            }
          }
          Ok(format!("removed={count}"))
        }
        "doctor" => {
          let index = KuntuIndex::open_shared(home, true, "cli").map_err(|e| e.to_string())?;
          index.doctor().map_err(|e| e.to_string())
        }
        "import" => {
          let mut index = KuntuIndex::open_shared(home, false, "cli").map_err(|e| e.to_string())?;
          if !index.acquire_writer().map_err(|e| e.to_string())? {
            return Err("Indexer lease busy.".into());
          }
          let stats = index
            .import_v2(
              source
                .as_deref()
                .ok_or_else(|| "Missing import source.".to_string())?,
            )
            .map_err(|e| e.to_string())?;
          Ok(format_rebuild_stats(&stats))
        }
        _ => Err("Invalid store operation.".into()),
      }
    }
    CommandSpec::Search { query, json } => {
      let search =
        search_with_provider(&config, &core, &query, spec.provider, db_path, store_mode)?;
      if json {
        Ok(format_results_json(&search.results))
      } else {
        Ok(format_results_text(&search.results))
      }
    }
    CommandSpec::Explain { path, query } => {
      let explain = core.explain_path(&path, query.as_ref());
      Ok(format_explain_text(&explain))
    }
    CommandSpec::Bench { query } => {
      let started = Instant::now();
      let search =
        search_with_provider(&config, &core, &query, spec.provider, db_path, store_mode)?;
      Ok(format_bench_stats(
        search.candidate_count,
        search.results.len(),
        started.elapsed().as_millis(),
      ))
    }
    CommandSpec::Index(command) => run_index_command(&config, db_path, command, store_mode),
    CommandSpec::Watch { duration_ms } => {
      run_watch_command(&config, db_path, duration_ms, store_mode)
    }
    CommandSpec::Daemon {
      addr,
      duration_ms,
      allow_non_loopback,
    } => run_daemon_command(
      &config,
      db_path,
      addr,
      duration_ms,
      allow_non_loopback,
      store_mode,
    ),
  }
}

fn search_with_provider(
  config: &SearchConfig,
  core: &SearchEngineCore,
  query: &SearchQuery,
  provider: SearchProvider,
  db_path: Option<&Path>,
  store_mode: StoreMode,
) -> Result<SearchRun, String> {
  match provider {
    SearchProvider::Spotlight => search_spotlight(config, core, query),
    SearchProvider::Sqlite => {
      let path = db_path.ok_or_else(|| "--db is required for sqlite provider".to_string())?;
      search_sqlite(config, query, path, store_mode)
    }
    SearchProvider::Auto => {
      if let Some(path) = db_path {
        let search = search_sqlite(config, query, path, store_mode)?;
        if !search.results.is_empty() {
          return Ok(search);
        }
      }
      search_spotlight(config, core, query)
    }
  }
}

fn search_spotlight(
  config: &SearchConfig,
  core: &SearchEngineCore,
  query: &SearchQuery,
) -> Result<SearchRun, String> {
  let provider = SpotlightProvider::default();
  let candidates = provider
    .search_candidates(config, query)
    .map_err(|err| err.to_string())?;
  let candidate_count = candidates.len();
  Ok(SearchRun {
    results: core.search_candidates(query, candidates),
    candidate_count,
  })
}

fn open_cli_index(
  path: &Path,
  read_only: bool,
  mode: StoreMode,
) -> kuntu_index::Result<KuntuIndex> {
  if mode == StoreMode::Shared {
    KuntuIndex::open_shared(
      Home::open(
        path
          .parent()
          .ok_or_else(|| std::io::Error::other("Missing home."))?,
      )?,
      read_only,
      "cli",
    )
  } else if read_only {
    KuntuIndex::open_read_only(path)
  } else {
    KuntuIndex::open(path)
  }
}
fn search_sqlite(
  config: &SearchConfig,
  query: &SearchQuery,
  db_path: &Path,
  store_mode: StoreMode,
) -> Result<SearchRun, String> {
  let index = open_cli_index(db_path, true, store_mode).map_err(|err| err.to_string())?;
  let outcome = index
    .search_with_metrics(config, query)
    .map_err(|err| err.to_string())?;
  Ok(SearchRun {
    results: outcome.results,
    candidate_count: outcome.candidate_count,
  })
}

fn run_index_command(
  config: &SearchConfig,
  db_path: Option<&Path>,
  command: IndexCommand,
  store_mode: StoreMode,
) -> Result<String, String> {
  let path = db_path.ok_or_else(|| "--db is required for index commands".to_string())?;
  let mut index = open_cli_index(path, false, store_mode).map_err(|err| err.to_string())?;
  if index.is_shared() {
    for root in &config.roots {
      index.register_root(root).map_err(|e| e.to_string())?;
    }
    if !matches!(command, IndexCommand::Status)
      && !index.acquire_writer().map_err(|e| e.to_string())?
    {
      let kind = match command {
        IndexCommand::Rebuild => "rebuild",
        IndexCommand::Refresh => "refresh",
        IndexCommand::Repair => "repair",
        IndexCommand::Status => unreachable!(),
      };
      let id = index.enqueue(kind, config).map_err(|e| e.to_string())?;
      return Ok(format!("queued={id}"));
    }
  }
  match command {
    IndexCommand::Rebuild => {
      let stats = index.rebuild_index(config).map_err(|err| err.to_string())?;
      Ok(format_rebuild_stats(&stats))
    }
    IndexCommand::Refresh => {
      let stats = index.refresh_index(config).map_err(|err| err.to_string())?;
      Ok(format_refresh_stats(&stats))
    }
    IndexCommand::Repair => {
      let stats = index.repair_index(config).map_err(|err| err.to_string())?;
      Ok(format_repair_stats(&stats))
    }
    IndexCommand::Status => {
      let status = index.status().map_err(|err| err.to_string())?;
      let states = status
        .iter()
        .map(|r| {
          index
            .root_state_name(&r.path)
            .map(|s| format!("{} status={s}", r.path.display()))
            .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
      Ok(format!("{}\n{}", format_status(&status), states.join("\n")))
    }
  }
}

fn run_watch_command(
  config: &SearchConfig,
  db_path: Option<&Path>,
  duration_ms: u64,
  store_mode: StoreMode,
) -> Result<String, String> {
  let path = db_path.ok_or_else(|| "--db is required for watch".to_string())?;
  let mut index = open_cli_index(path, false, store_mode).map_err(|err| err.to_string())?;
  if index.is_shared() {
    for root in &config.roots {
      index.register_root(root).map_err(|e| e.to_string())?;
    }
    if !index.acquire_writer().map_err(|e| e.to_string())? {
      return Err("Indexer lease busy; watch requires a holder.".into());
    }
  }
  let options = WatchOptions::new(config.clone())
    .with_duration(Duration::from_millis(duration_ms))
    .with_poll_interval(Duration::from_millis(100));
  let stats = run_polling_watch(&options, |_event| index.refresh_index(config).map(|_| ()))
    .map_err(|err| err.to_string())?;
  if stats.dirty {
    for root in config.roots.iter().filter(|root| root.enabled) {
      index.mark_root_dirty(root).map_err(|err| err.to_string())?;
    }
  }
  Ok(format_watch_stats(&stats))
}

fn run_daemon_command(
  config: &SearchConfig,
  db_path: Option<&Path>,
  addr: String,
  duration_ms: Option<u64>,
  allow_non_loopback: bool,
  store_mode: StoreMode,
) -> Result<String, String> {
  let path = db_path.ok_or_else(|| "--db is required for daemon".to_string())?;
  let stats = serve(DaemonConfig {
    db_path: path.to_path_buf(),
    store_mode,
    addr,
    duration: duration_ms.map(Duration::from_millis),
    max_requests: None,
    allowed_roots: config.roots.clone(),
    allow_non_loopback,
  })
  .map_err(|err| err.to_string())?;
  Ok(format_daemon_stats(&stats))
}

#[cfg(test)]
mod tests {
  use std::fs;
  use std::path::PathBuf;
  use std::time::{SystemTime, UNIX_EPOCH};

  use super::*;

  fn explicit_private_fixture(label: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let parent = temp_dir(label);
    fs::create_dir_all(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let root = parent.join("files");
    fs::create_dir(&root).unwrap();
    let path = parent.join("index.v3.db");
    let index = KuntuIndex::open(&path).unwrap();
    index
      .register_root(&kuntu_core::SearchRoot::new(&root))
      .unwrap();
    drop(index);
    (path, root)
  }
  #[test]
  fn explicit_index_v3_basename_stays_private_in_cli() {
    let (path, _) = explicit_private_fixture("explicit-private-cli");
    let result = run(vec![
      "index".into(),
      "status".into(),
      "--db".into(),
      path.to_string_lossy().into_owned(),
    ]);
    assert!(
      result.is_ok(),
      "explicit private path was treated as shared: {result:?}"
    );
    assert!(!path.parent().unwrap().join("store.json").exists());
  }
  #[test]
  fn explicit_index_v3_basename_stays_private_in_daemon() {
    let (path, root) = explicit_private_fixture("explicit-private-daemon");
    let result = run(vec![
      "daemon".into(),
      "--db".into(),
      path.to_string_lossy().into_owned(),
      "--root".into(),
      root.to_string_lossy().into_owned(),
      "--addr".into(),
      "127.0.0.1:0".into(),
      "--duration-ms".into(),
      "1".into(),
    ]);
    assert!(
      result.is_ok(),
      "daemon inferred shared mode from basename: {result:?}"
    );
    assert!(!path.parent().unwrap().join("store.json").exists());
  }

  #[test]
  fn bench_reports_candidate_and_result_counts_for_sqlite() {
    let root = temp_dir("bench-root");
    let db_path = temp_dir("bench-db").with_extension("sqlite");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("needlebench-target.md"), "needle\n").unwrap();
    fs::write(root.join("other-target.md"), "other\n").unwrap();

    run(vec![
      "index".to_string(),
      "rebuild".to_string(),
      "--root".to_string(),
      root.to_string_lossy().into_owned(),
      "--db".to_string(),
      db_path.to_string_lossy().into_owned(),
    ])
    .unwrap();
    let output = run(vec![
      "bench".to_string(),
      "needlebench".to_string(),
      "--root".to_string(),
      root.to_string_lossy().into_owned(),
      "--provider".to_string(),
      "sqlite".to_string(),
      "--db".to_string(),
      db_path.to_string_lossy().into_owned(),
    ])
    .unwrap();

    cleanup(&root, &db_path);

    assert!(output.contains("candidates=1"));
    assert!(output.contains("results=1"));
    assert!(output.contains("elapsed_ms="));
  }

  fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    std::env::temp_dir().join(format!("kuntu-cli-{name}-{nonce}"))
  }

  fn cleanup(root: &Path, db_path: &Path) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(db_path);
    let _ = fs::remove_file(db_path.with_extension("sqlite-shm"));
    let _ = fs::remove_file(db_path.with_extension("sqlite-wal"));
  }
}
