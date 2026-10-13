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
      let start = Instant::now();
      let mut samples = Vec::new();
      let mut errors = 0;
      while start.elapsed() < Duration::from_secs(60) {
        let t = Instant::now();
        if (if samples.len().is_multiple_of(2) {
          index.point_probe()
        } else {
          index.range_probe()
        })
        .is_err()
        {
          errors += 1;
        }
        samples.push(t.elapsed().as_micros());
        std::thread::sleep(Duration::from_micros(500));
      }
      samples.sort_unstable();
      let p99 = samples[(samples.len() * 99 / 100).min(samples.len() - 1)];
      let max = *samples.last().unwrap();
      println!(
        "{{\"queries\":{},\"errors\":{errors},\"p99_us\":{p99},\"max_us\":{max}}}",
        samples.len()
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
