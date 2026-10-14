//! Test-only fixtures for the release-mode latency gate and the recall tests.
//!
//! `open_or_build` generates (and caches on disk) a *realistic* store: Zipf
//! distributed vocabulary, a pool of nested directories, one to four name
//! tokens joined by mixed separators, a skewed extension mix, and the root
//! path components that every real row carries. Rows are written with
//! guarded v3 batches through the same SQLite connection and posting writer,
//! with the same term rules as `insert_terms_tx` (name tokens = field 1,
//! extension = field 2, every path component token = field 3).
//!
//! The single-term fixture in `run_bounded_fixture` is pathological (one
//! posting list of N rows); this one is what a real home directory looks like.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use kuntu_core::ranking::tokenize;
use kuntu_core::{SearchConfig, SearchRoot};
use rusqlite::params;

use super::{block_on, remove_database_files, KuntuIndex};

/// xorshift64: deterministic across platforms and runs.
pub struct Rng(u64);

impl Rng {
  pub fn new(seed: u64) -> Self {
    Self(seed)
  }

  pub fn next_u64(&mut self) -> u64 {
    self.0 ^= self.0 << 13;
    self.0 ^= self.0 >> 7;
    self.0 ^= self.0 << 17;
    self.0
  }

  fn unit(&mut self) -> f64 {
    (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
  }

  /// Log-uniform rank in `0..n`, i.e. a Zipf(1) distribution.
  pub fn zipf(&mut self, n: usize) -> usize {
    (((n as f64).ln() * self.unit()).exp() as usize)
      .saturating_sub(1)
      .min(n - 1)
  }
}

const SYLLABLES: [&str; 40] = [
  "ba", "ko", "ri", "tu", "men", "sa", "lo", "vi", "dar", "pe", "qua", "non", "fi", "gra", "hu",
  "jo", "ka", "lem", "mo", "ne", "or", "pla", "ro", "sti", "tra", "un", "ve", "wi", "xa", "yo",
  "zen", "con", "dis", "ex", "inf", "pro", "re", "sub", "tel", "ult",
];

const EXTENSIONS: [&str; 30] = [
  "ts", "js", "json", "md", "rs", "png", "jpg", "pdf", "txt", "log", "toml", "yaml", "html", "css",
  "svelte", "py", "go", "swift", "mp4", "zip", "csv", "docx", "xlsx", "heic", "dat", "lock", "map",
  "sh", "c", "h",
];

fn word(index: usize) -> String {
  let mut rest = index + 1;
  let mut word = String::new();
  let mut syllables = 0;
  while rest > 0 || syllables < 2 {
    word.push_str(SYLLABLES[rest % SYLLABLES.len()]);
    rest /= SYLLABLES.len();
    syllables += 1;
  }
  word
}

/// One row to plant: an absolute path and whether it is a directory.
#[derive(Debug, Clone)]
pub struct RowSpec {
  pub path: String,
  pub is_dir: bool,
}

/// Query texts chosen from the generated data, with ground truth.
#[derive(Debug, Clone)]
pub struct FixtureQueries {
  /// A token on 3 to 10 rows.
  pub rare: String,
  /// A token on about rows/3300 .. rows/1000 rows.
  pub mid: String,
  /// A token on about rows/50 .. rows/12 rows.
  pub broad: String,
  pub prefix3: String,
  pub prefix1: String,
  /// A token present on every row (a component of the fixture root).
  pub all_rows: String,
  /// `"<mid> ts"`: a selective token plus the most common extension.
  pub mixed: String,
  /// Rows whose path has a token starting with `mid` and one starting with `ts`.
  pub mixed_truth: usize,
}

pub struct Fixture {
  pub index: KuntuIndex,
  pub config: SearchConfig,
  pub rows: usize,
  pub queries: FixtureQueries,
  /// `Some` when this call built the fixture, `None` when it was reused.
  pub build_secs: Option<f64>,
  pub db_bytes: u64,
}

pub fn generate_rows(root: &str, rows: usize) -> Vec<RowSpec> {
  let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
  let vocabulary = 40_000;
  let directory_target = (rows / 12).max(1);
  let mut directories: Vec<(String, u32)> = vec![(root.to_string(), 0)];
  let mut seen: HashSet<String> = HashSet::new();
  let mut specs: Vec<RowSpec> = Vec::with_capacity(rows);
  while directories.len() < directory_target {
    let parent = (rng.next_u64() as usize) % directories.len();
    if directories[parent].1 >= 8 {
      continue;
    }
    let depth = directories[parent].1 + 1;
    let path = format!("{}/{}", directories[parent].0, word(rng.zipf(vocabulary)));
    if seen.insert(path.clone()) {
      specs.push(RowSpec {
        path: path.clone(),
        is_dir: true,
      });
      directories.push((path, depth));
    }
  }
  while specs.len() < rows {
    let directory = (rng.next_u64() as usize) % directories.len();
    let tokens = 1 + (rng.next_u64() % 4) as usize;
    let separator = ["-", "_", ".", " "][(rng.next_u64() % 4) as usize];
    let mut name = (0..tokens)
      .map(|_| word(rng.zipf(vocabulary)))
      .collect::<Vec<_>>()
      .join(separator);
    if rng.next_u64().is_multiple_of(5) {
      name.push_str(&(rng.next_u64() % 1000).to_string());
    }
    let extension = EXTENSIONS[rng.zipf(EXTENSIONS.len())];
    let path = format!("{}/{name}.{extension}", directories[directory].0);
    if seen.insert(path.clone()) {
      specs.push(RowSpec {
        path,
        is_dir: false,
      });
    }
  }
  specs
}

/// Plant the realistic fixture through v3's actual entry/posting writer.
pub fn insert_rows(index: &mut KuntuIndex, root_id: i64, rows: &[RowSpec], first_id: i64) {
  use super::{block_on, insert_terms_tx};
  let mut next = first_id;
  for batch in rows.chunks(5000) {
    let tx = index.conn.begin_writer_txn(false).unwrap();
    for row in batch {
      let path = Path::new(&row.path);
      tx.conn.raw.prepare_cached("INSERT INTO entries(id,root_id,path,name,extension,kind,size) VALUES(?1,?2,?3,?4,?5,?6,8)").unwrap().execute(params![next,root_id,row.path,path.file_name().and_then(|v|v.to_str()).unwrap_or_default(),path.extension().and_then(|v|v.to_str()),if row.is_dir{2}else{1}]).unwrap();
      block_on(insert_terms_tx(&tx, next, path)).unwrap();
      next += 1;
    }
    block_on(tx.commit()).unwrap();
  }
}

fn pick(counts: &HashMap<String, u32>, low: u32, high: u32) -> String {
  let mut candidates: Vec<(&String, &u32)> = counts
    .iter()
    .filter(|(token, count)| {
      **count >= low
        && **count <= high
        && token.len() >= 4
        && token.bytes().all(|byte| byte.is_ascii_lowercase())
    })
    .collect();
  candidates.sort_by(|left, right| left.1.cmp(right.1).then_with(|| left.0.cmp(right.0)));
  candidates
    .get(candidates.len() / 2)
    .map(|(token, _)| (*token).clone())
    .expect("the fixture has a token in the requested frequency band")
}

fn choose_queries(specs: &[RowSpec], rows: usize) -> FixtureQueries {
  let mut counts: HashMap<String, u32> = HashMap::new();
  let mut row_tokens: Vec<Vec<String>> = Vec::with_capacity(specs.len());
  for spec in specs {
    let mut tokens = tokenize(&spec.path);
    tokens.sort();
    tokens.dedup();
    for token in &tokens {
      *counts.entry(token.clone()).or_insert(0) += 1;
    }
    row_tokens.push(tokens);
  }
  let rows_u32 = rows as u32;
  let rare = pick(&counts, 3, 10);
  let mid = pick(
    &counts,
    (rows_u32 / 3300).max(20),
    (rows_u32 / 1000).max(60),
  );
  let broad = pick(&counts, rows_u32 / 50, rows_u32 / 12);
  let has_prefix =
    |tokens: &[String], prefix: &str| tokens.iter().any(|token| token.starts_with(prefix));
  let mixed_truth = row_tokens
    .iter()
    .filter(|tokens| has_prefix(tokens, &mid) && has_prefix(tokens, "ts"))
    .count();
  FixtureQueries {
    prefix3: broad[..3].to_string(),
    prefix1: broad[..1].to_string(),
    mixed: format!("{mid} ts"),
    all_rows: "kuntu".to_string(),
    mixed_truth,
    rare,
    mid,
    broad,
  }
}

fn write_queries(path: &Path, rows: usize, queries: &FixtureQueries) {
  let text = format!(
    "rows={rows}\nrare={}\nmid={}\nbroad={}\nprefix3={}\nprefix1={}\nall_rows={}\nmixed={}\nmixed_truth={}\n",
    queries.rare,
    queries.mid,
    queries.broad,
    queries.prefix3,
    queries.prefix1,
    queries.all_rows,
    queries.mixed,
    queries.mixed_truth
  );
  fs::write(path, text).unwrap();
}

fn read_queries(path: &Path, rows: usize) -> Option<FixtureQueries> {
  let text = fs::read_to_string(path).ok()?;
  let values: HashMap<&str, &str> = text
    .lines()
    .filter_map(|line| line.split_once('='))
    .collect();
  if values.get("rows")?.parse::<usize>().ok()? != rows {
    return None;
  }
  Some(FixtureQueries {
    rare: values.get("rare")?.to_string(),
    mid: values.get("mid")?.to_string(),
    broad: values.get("broad")?.to_string(),
    prefix3: values.get("prefix3")?.to_string(),
    prefix1: values.get("prefix1")?.to_string(),
    all_rows: values.get("all_rows")?.to_string(),
    mixed: values.get("mixed")?.to_string(),
    mixed_truth: values.get("mixed_truth")?.parse().ok()?,
  })
}

/// Open the cached fixture for `rows`, building it first when it is missing.
/// The cache lives in `$KUNTU_BENCH_DIR` (default: a `kuntu-bench` directory
/// under the system temp dir) and is reused across runs; delete the directory
/// to force a rebuild. The queries file is written last, so an interrupted
/// build is rebuilt.
pub fn open_or_build(rows: usize) -> Fixture {
  assert!(
    rows >= 20_000,
    "the frequency bands need at least 20,000 rows"
  );
  let base = std::env::var_os("KUNTU_BENCH_DIR")
    .map(PathBuf::from)
    .unwrap_or_else(|| std::env::temp_dir().join("kuntu-bench"));
  fs::create_dir_all(&base).unwrap();
  let root = base.join(format!("kuntu-root-{rows}"));
  fs::create_dir_all(&root).unwrap();
  let root = PathBuf::from("/Users/kuntu/fixture");
  let db_path = base.join(format!("realistic-v3-rev2-compact-{rows}.sqlite"));
  let queries_path = base.join(format!("realistic-v3-rev2-compact-{rows}.queries"));
  let config = SearchConfig {
    roots: vec![SearchRoot::new(&root)],
  };

  if db_path.exists() {
    if let Some(queries) = read_queries(&queries_path, rows) {
      let index = KuntuIndex::open(&db_path).unwrap();
      let db_bytes = fs::metadata(&db_path).map(|meta| meta.len()).unwrap_or(0);
      return Fixture {
        index,
        config,
        rows,
        queries,
        build_secs: None,
        db_bytes,
      };
    }
  }

  remove_database_files(&db_path);
  let _ = fs::remove_file(&queries_path);
  let started = Instant::now();
  let specs = generate_rows(&root.to_string_lossy(), rows);
  let queries = choose_queries(&specs, rows);
  {
    let mut index = KuntuIndex::open(&db_path).unwrap();
    let root_id = index.register_root(&config.roots[0]).unwrap();
    let tx = index.conn.begin_writer_txn(false).unwrap();
    tx.conn
      .raw
      .execute(
        "UPDATE root_state SET status='ready',dirty=0 WHERE root_id=?1",
        params![root_id],
      )
      .unwrap();
    tx.conn.raw.execute("DELETE FROM entries", []).unwrap();
    block_on(tx.commit()).unwrap();
    let root_id: i64 = block_on(async {
      let mut found = index
        .conn
        .query(
          "SELECT id FROM roots WHERE path = ?1",
          params![root.to_string_lossy()],
        )
        .await
        .unwrap();
      found.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    });
    insert_rows(&mut index, root_id, &specs, 1);
  }
  write_queries(&queries_path, rows, &queries);
  let build_secs = started.elapsed().as_secs_f64();
  let index = KuntuIndex::open(&db_path).unwrap();
  let db_bytes = fs::metadata(&db_path).map(|meta| meta.len()).unwrap_or(0);
  eprintln!("FIXTURE built rows={rows} in {build_secs:.1} s, {db_bytes} bytes at {db_path:?}");
  Fixture {
    index,
    config,
    rows,
    queries,
    build_secs: Some(build_secs),
    db_bytes,
  }
}

/// Measured runs of one gate class, after the untimed warm-up.
pub const TIMED_RUNS: usize = 3;

/// Timing policy of the latency gate: one untimed warm-up run, then the best
/// of [`TIMED_RUNS`] measured runs. The warm-up absorbs the cold page cache
/// and the freshly opened database; no measured run is abandoned early,
/// because an early break would let the first cold run decide the gate
/// instead of the warm runs the policy is about (review P2).
pub fn measure_warm_best(mut run: impl FnMut() -> std::time::Duration) -> std::time::Duration {
  run();
  let mut best = std::time::Duration::MAX;
  for _ in 0..TIMED_RUNS {
    best = best.min(run());
  }
  best
}
