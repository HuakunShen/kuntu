//! Foreground process probe for real crash, skew, lease and stress evidence.
use kuntu_core::{SearchConfig, SearchQuery, SearchRoot};
use kuntu_index::{KuntuIndex, WriterService};
use kuntu_store::Home;
use std::path::Path;
use std::time::{Duration, Instant};
fn mark(text: &str) -> std::io::Result<()> {
  if let Ok(p) = std::env::var("KUNTU_FAULT_MARKER") {
    std::fs::write(p, text)?;
  }
  Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
  let a: Vec<_> = std::env::args().collect();
  let home = Home::open(Path::new(&a[2]))?;
  if a[1] == "create-v2" {
    let path = home.path.join("source.v2.db");
    let db =
      futures::executor::block_on(turso::Builder::new_local(path.to_str().unwrap()).build())?;
    let conn = db.connect()?;
    futures::executor::block_on(conn.execute_batch(kuntu_legacy_index::SCHEMA_DDL))?;
    futures::executor::block_on(conn.execute_batch("PRAGMA user_version=2;"))?;
    futures::executor::block_on(conn.execute(
      "INSERT INTO roots(path,enabled,updated_at) VALUES(?1,1,0)",
      turso::params![a[3].clone()],
    ))?;
    println!("{}", path.display());
    return Ok(());
  }
  if a[1] == "identity" {
    let index = KuntuIndex::open_shared(home.clone(), false, "probe")?;
    drop(index);
    println!("{}", home.id()?);
    return Ok(());
  }
  if a[1] == "gc-init" {
    let guard = home.lifetime(2)?;
    drop(guard);
    for ext in ["", "-wal", "-shm"] {
      use std::io::Write;
      home
        .file(&format!("index.v2.db{ext}"), true)?
        .write_all(b"old epoch member")?;
    }
    assert!(!home.gc_epoch(2, 1)?);
    return Ok(());
  }
  if a[1] == "gc" {
    assert!(home.gc_epoch(2, 31 * 86400)?);
    return Ok(());
  }
  if a[1] == "reuse-old" {
    let _guard = home.lifetime(2)?;
    for ext in ["", "-wal", "-shm"] {
      assert!(!home.exists(&format!("index.v2.db{ext}"))?);
    }
    assert!(!home.exists("gc-2.json")?);
    println!("gc resumed before epoch reuse");
    return Ok(());
  }
  if a[1] == "quarantine" {
    home.quarantine(3)?;
    return Ok(());
  }
  if a[1] == "poll" {
    let service = WriterService::start(home);
    let start = Instant::now();
    while !service.is_holder() {
      if start.elapsed() > Duration::from_secs(20) {
        return Err("Takeover timeout.".into());
      }
      std::thread::sleep(Duration::from_millis(10));
    }
    println!("takeover_ms={}", start.elapsed().as_millis());
    mark("holder")?;
    return Ok(());
  }
  let minor = a.get(4).and_then(|v| v.parse::<i64>().ok()).unwrap_or(1);
  let readonly = matches!(a[1].as_str(), "reader" | "hold-reader" | "skew-reader");
  let mut index = KuntuIndex::open_shared_minor(home.clone(), readonly, "probe", minor)?;
  let config = SearchConfig {
    roots: vec![SearchRoot::new(a.get(3).ok_or("Missing root.")?)],
  };
  match a[1].as_str() {
    "init-permissive" => {
      let mut root = config.roots[0].clone();
      root.include_hidden = true;
      root.include_ignored = true;
      index.register_root(&root)?;
    }
    "init" => {
      index.register_root(&config.roots[0])?;
    }
    "scan" => {
      if !index.acquire_writer()? {
        return Err("Lease busy.".into());
      }
      let s = index.rebuild(&config)?;
      println!(
        "entries={} generation={} check={}",
        s.entries,
        index.generation_probe()?,
        index.doctor()?
      );
    }
    "inspect" => {
      println!(
        "entries={} generation={} check={} access={:?}",
        index.count_entries()?,
        index.generation_probe()?,
        index.doctor()?,
        index.access()
      );
    }
    "hold" | "hold-reader" => {
      if a[1] == "hold" && !index.acquire_writer()? {
        return Err("Lease busy.".into());
      }
      mark("held")?;
      std::thread::sleep(Duration::from_secs(120));
    }
    "exec-holder" => {
      if !index.acquire_writer()? {
        return Err("Lease busy.".into());
      }
      mark("held")?;
      let child = std::process::Command::new("/bin/sleep")
        .arg("20")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
      println!("child_pid={}", child.id());
      // Returning drops the lease while the exec child is still alive.
    }
    "skew-reader" => {
      println!("access={:?} check={}", index.access(), index.doctor()?);
    }
    "skew-writer" => {
      let acquired = index.acquire_writer()?;
      println!("acquired={acquired} access={:?}", index.access());
    }
    "read" => {
      println!(
        "{:?}",
        index
          .search_with_metrics(&config, &SearchQuery::new("file"))?
          .coverage
      );
    }
    "reader" => {
      let committed = index.count_entries()?;
      if committed == 0 {
        return Err("reader requires committed entries before sampling".into());
      }
      let start = Instant::now();
      let start_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_micros();
      let done = std::env::var_os("KUNTU_SAMPLING_DONE").map(std::path::PathBuf::from);
      let mut samples = Vec::new();
      let mut errors = 0;
      let mut cpu_total = 0_u128;
      let mut wall_total = 0_u128;
      let mut io_total = 0_u128;
      let mut sleep_total = 0_u128;
      let mut retries = 0_u64;
      let mut outliers = Vec::new();
      while start.elapsed() < Duration::from_secs(60) || done.as_ref().is_some_and(|p| !p.exists())
      {
        let timestamp = std::time::SystemTime::now()
          .duration_since(std::time::UNIX_EPOCH)?
          .as_micros();
        let cpu = kuntu_vfs::thread_cpu_ns();
        kuntu_vfs::begin_probe();
        let t = Instant::now();
        let kind = if samples.len().is_multiple_of(2) {
          "point"
        } else {
          "range"
        };
        let result = if kind == "point" {
          index.point_probe()
        } else {
          index.range_probe()
        };
        let wall = t.elapsed().as_micros();
        let measured = kuntu_vfs::end_probe();
        let cpu = kuntu_vfs::thread_cpu_ns().saturating_sub(cpu) / 1000;
        if !matches!(result, Ok(n) if n > 0) {
          errors += 1;
        }
        cpu_total += u128::from(cpu);
        wall_total += wall;
        io_total += u128::from(measured.io_ns);
        sleep_total += u128::from(measured.sleep_ns);
        retries += measured.retries;
        if wall > 5000 {
          outliers.push(serde_json::json!({"timestamp_us":timestamp,"kind":kind,"wall_us":wall,"cpu_us":cpu,
            "sqlite_retries":measured.retries,"sqlite_sleep_us":measured.sleep_ns/1000,"storage_syscall_us":measured.io_ns/1000,"storage_syscalls":measured.io_calls}));
        }
        samples.push(wall);
        std::thread::sleep(Duration::from_micros(500));
      }
      let duration = start.elapsed().as_micros();
      let end_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_micros();
      samples.sort_unstable();
      let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
      let max = *samples.last().unwrap();
      println!(
        "{}",
        serde_json::json!({"queries":samples.len(),"errors":errors,"p99_us":p99,"max_us":max,
        "committed_entries_at_start":committed,"sampling_start_us":start_us,"sampling_end_us":end_us,"sampling_duration_us":duration,
        "probe_wall_total_us":wall_total,"cpu_total_us":cpu_total,"sqlite_retries":retries,
        "sqlite_sleep_total_us":sleep_total/1000,"storage_syscall_total_us":io_total/1000,"outliers":outliers})
      );
    }
    "writer" => {
      let start = Instant::now();
      let mut commits = 0;
      let mut blocked = 0;
      while start.elapsed() < Duration::from_secs(60) {
        if index.acquire_writer()? {
          index.rebuild(&config)?;
          commits += 1;
        } else {
          index.enqueue("refresh", &config)?;
          blocked += 1;
          std::thread::sleep(Duration::from_millis(10));
        }
      }
      println!("{{\"scans\":{commits},\"inbox_writes\":{blocked}}}");
    }
    _ => return Err("Unknown probe operation.".into()),
  }
  Ok(())
}
