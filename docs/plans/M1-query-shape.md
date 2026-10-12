# Kuntu M1: query-shape fix (v0.5.0)

Executable hand-off plan for an implementation worker. Source of the design and all numbers: `RESEARCH.md` and `PLAN.md` (milestone M1) in `/Volumes/Portable2TB/tmp/kuntu/`. The owner accepted the recommended defaults for the open decisions (Kunkun DECISIONS #159): SQLite for the later shared store, M1 first on turso, fetch budget 5,000.

**Replay status.** Every edit block below was applied mechanically to a clean clone of `main` (`4c5dbfd`) and every RED/GREEN step was run as written; the measured results are in "Replay results" at the end of this file.

## Goal

Make search fast and exact at 1,000,000 entries **without changing the engine, the schema, any consumer pin, or the public NAPI/JS surface**:

1. Candidate fetch by rowid only, with scope, kind, extension and policy applied in Rust before ranking and before the result window.
2. Stage-1 range scans on the covering primary-key index (no `DISTINCT` / `ORDER BY`).
3. Non-ASCII tokens use a range instead of `LIKE` (a miss no longer scans the whole terms table).
4. The rarest token drives multi-token queries; the others are verified against the path in Rust (exact recall instead of silently truncated intersections).
5. A fetch budget (default 5,000, hard cap 50,000) bounds the work and is reported through `CandidateCoverage::Truncated`.
6. A committed realistic 1M-row fixture generator and a release-mode latency gate, replacing a benchmark that currently fails.

## Baseline (why this is needed)

Measured on `4c5dbfd`, release, Apple M4 Pro, external disk, turso 0.7.2 (details in `RESEARCH.md` sections 2 and 3):

| Fact | Number |
|---|---|
| `just benchmark`, 1M single-term test | **fails**: "bounded broad query took 1005.7 s", ceiling 300 s |
| Same, 60k rows, broad query (50,000 candidates) | 31.3 s (README says about 2.3 s) |
| Realistic 1M fixture, ASCII miss | 0.2 to 1.3 ms (the README's "7.5 s miss" does not reproduce; the range is a seek) |
| Realistic 1M fixture, non-ASCII miss | 15.5 s and 21.5 s (`LIKE` scans the terms table) |
| Realistic 1M fixture, 205-candidate query | 2.5 to 2.7 s |
| Realistic 1M fixture, 20,270-candidate query | 298.7 s |
| `entries JOIN roots WHERE id IN (400 ids) AND <predicates>` at 1M rows | 4.1 s; the bare `WHERE id IN (...)` takes 2.7 ms |
| Multi-token `tujoko ts`-style query | returns 8 rows flagged `Truncated`; the true answer is 33 |

Root causes: (1) the candidate fetch makes turso scan the whole root's index for every batch because `root_id`/`extension` predicates sit beside the id list; (2) `LIKE 'tok%'` for non-ASCII tokens; (3) per-token 50,000-id caps intersected in Rust lose rows when one token is selective and another is common.

## Rules

* Work on a new branch `feat/m1-query-shape` created from `main`. Read this file from the plan branch: `git show docs/m1-query-shape:docs/plans/M1-query-shape.md`. **Never push, merge, tag, or touch another branch.** The owner tags `v0.5.0`.
* One commit per task, message given in each task. Run `cargo fmt --all` before every commit.
* Test-first: apply the tests, run them, confirm each RED fails **as stated** (a different failure means stop and report), then apply the implementation and confirm GREEN.
* Run every command in the foreground. Use `--release` for all `kuntu-index` test runs: a debug turso build is too slow for the fixtures, and the first release build takes about 12 minutes (later builds are incremental). Keep scratch data out of `/private/tmp`; use `/Volumes/Portable2TB/tmp/kuntu-bench` (or any directory outside the repo) for `KUNTU_BENCH_DIR`.
* Report exact evidence, known failures, and not-run areas. A passing checklist is not proof; the numbers are.
* Edits are given as exact anchors. Each HTML comment that starts with the word `apply` (in a one-line JSON form) is the machine-readable form of the edit that follows it (`create`, `insert-before` an anchor string, `replace-range` from a start string up to, not including, an end string, or `append-to-tests-module`, which inserts before the final `}` of `mod tests`). If you apply them by hand, the captions say the same thing. Do not apply unified diffs.

## Exact write set

* `crates/kuntu-index/src/lib.rs` (edits only at the anchors below)
* `crates/kuntu-index/src/fixture.rs` (new, `#[cfg(test)]` only)
* `justfile`
* `README.md` (one `Revisions` row)

Nothing else. In particular no `Cargo.toml`, `Cargo.lock`, `kuntu-core`, `kuntu-napi` (including the generated `index.js`/`index.d.ts`), `kuntu-daemon`, `kuntu-cli`, CI workflow, or schema changes.

## Constraints

* **No engine change**: `turso =0.7.2`, `default-features = false`, unchanged. No new dependencies, not even dev-dependencies.
* **No schema change**: `SCHEMA_VERSION` stays 2; the DDL and every write path (`rebuild`, `refresh`, `repair`, `insert_terms_tx`, ...) are untouched.
* **No public NAPI/JS surface change**: `KuntuIndex::search`, `search_with_metrics` and `IndexedSearchOutcome` keep their signatures and meaning. Additions are purely additive (`SearchOptions`, `DEFAULT_FETCH_BUDGET`, `KuntuIndex::search_with_options`). `SearchQuery` (in `kuntu-core`) gets no new field, because Xross builds it with a struct literal. `git diff main -- crates/kuntu-napi crates/kuntu-core` must be empty.
* **Keep the v0.4.0 semantics**: enabled-root scoping, requested kind (`include_files` / `include_directories`), extension filter (dotted or bare), hidden/ignored/sensitive policy including a root's own allowance, all applied **before ranking and before any result window**; `CandidateCoverage::Truncated` instead of pretending completeness; a query whose tokens match nothing returns an **empty, complete** result (no root scan fallback); equal scores tie-break by path.
* The only intended behaviour changes: `candidate_count` is now at most the fetch budget (default 5,000) instead of at most 50,000, and mixed-selectivity multi-token queries become exact instead of truncated.

## Review focus

1. Stage-2 admission (`CandidateScope::admit`) is a line-for-line port of the removed SQL predicates. Check `Other` kinds, a root's `include_hidden` / `include_ignored` allowance, dotted extensions, and that disabled or foreign roots never leak.
2. The fetch statement has **no predicate** besides `id IN (...)`. Anyone adding `root_id`, `extension`, `kind` or an indexed column there brings the whole-root scan back; the plan test pins it.
3. `path_has_prefix_tokens` must equal what the terms table encodes (tokens of every path component, ASCII-lowercased by `tokenize`). A mismatch silently drops results.
4. Budget accounting: the budget counts admitted candidates; `Truncated` means "stopped before every id was examined" (conservative; no extra fetch to prove more existed).
5. The driver choice never reports `Complete` after capping a list that was actually cut.
6. The latency gate ceilings: wide enough for a slow disk and a loaded machine, tight enough that the old behaviour fails by orders of magnitude.
7. No unsafe, no new dependencies, no change to what the NAPI addon exports.


## Task 1: realistic fixture helper and the latency gate (RED)

Why first: the existing benchmark prints nothing and is red, so nothing tells us whether a change helped. This task adds the generator and a gate that runs against the **current** code and fails the way production fails.

The generator (`fixture.rs`) is a test helper. It is deterministic (xorshift seed), Zipf-distributed (a 40,000-word syllable vocabulary), builds a pool of nested directories (about 1 in 12 rows), names with 1 to 4 tokens and mixed separators, a skewed 30-extension mix, and puts the fixture root's path components on every row (so `kuntu` is a token on all rows, like `users` in a real home directory). Rows are written with multi-row statements through the library's own turso connection and the same term rules as `insert_terms_tx`. It chooses the query tokens (rare, mid, broad, prefixes, a mixed-selectivity pair) and the ground-truth count for the mixed query from the generated data, and caches the database and the chosen queries on disk (`KUNTU_BENCH_DIR`, default `<tmp>/kuntu-bench`) so repeated runs skip the build.

The gate queries, in order, each run three times with the best run compared to its ceiling (ms): `miss` 25, `miss_non_ascii` 25, `rare` 50, `mid` 100, `broad` 400, `prefix3` 400, `mixed` 300 (also asserts the exact true count and `Complete`), `all_rows` 600 and `prefix1` 800 (both assert `Truncated`). It asserts class by class, so the first red class names the next missing fix.


Create `crates/kuntu-index/src/fixture.rs` (test-only helper):

<!-- apply {"task": 1, "file": "crates/kuntu-index/src/fixture.rs", "mode": "create"} -->
```rust
//! Test-only fixtures for the release-mode latency gate and the recall tests.
//!
//! `open_or_build` generates (and caches on disk) a *realistic* store: Zipf
//! distributed vocabulary, a pool of nested directories, one to four name
//! tokens joined by mixed separators, a skewed extension mix, and the root
//! path components that every real row carries. Rows are written with
//! multi-row statements through the same turso connection the library uses,
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
use turso::params;

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

fn sql_quote(value: &str) -> String {
  value.replace('\'', "''")
}

/// Terms exactly as `insert_terms_tx` derives them: (term, field, weight).
fn terms_for(spec: &RowSpec) -> Vec<(String, i64, i64)> {
  let path = Path::new(&spec.path);
  let name = path
    .file_name()
    .and_then(|value| value.to_str())
    .unwrap_or_default();
  let mut terms: Vec<(String, i64, i64)> = tokenize(name)
    .into_iter()
    .map(|term| (term, 1, 100))
    .collect();
  if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
    terms.push((extension.to_ascii_lowercase(), 2, 50));
  }
  for component in path
    .components()
    .filter_map(|component| component.as_os_str().to_str())
  {
    for term in tokenize(component) {
      terms.push((term, 3, 10));
    }
  }
  terms
}

/// Plant rows with consecutive ids starting at `first_id`, bypassing the
/// crawler. Values are generated literals, so multi-row statements keep a
/// million-row fixture to a few thousand round-trips.
pub fn insert_rows(index: &mut KuntuIndex, root_id: i64, rows: &[RowSpec], first_id: i64) {
  const ENTRIES_PER_STATEMENT: usize = 100;
  const ENTRIES_PER_TRANSACTION: usize = 25_000;
  block_on(async {
    let mut next_id = first_id;
    for transaction_rows in rows.chunks(ENTRIES_PER_TRANSACTION) {
      let tx = index.conn.transaction().await.unwrap();
      for batch in transaction_rows.chunks(ENTRIES_PER_STATEMENT) {
        let mut entries_sql = String::from(
          "INSERT INTO entries (id, root_id, path, path_lower, name, name_lower, extension, kind, size, mtime, hidden, ignored, sensitive, deleted, indexed_at) VALUES ",
        );
        let mut terms_sql =
          String::from("INSERT OR IGNORE INTO terms (term, entry_id, field, weight) VALUES ");
        let mut first_term = true;
        for (offset, spec) in batch.iter().enumerate() {
          let id = next_id;
          next_id += 1;
          let path = Path::new(&spec.path);
          let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
          let extension = if spec.is_dir {
            None
          } else {
            path
              .extension()
              .and_then(|value| value.to_str())
              .map(|value| value.to_ascii_lowercase())
          };
          let extension_sql = extension
            .map(|value| format!("'{}'", sql_quote(&value)))
            .unwrap_or_else(|| "NULL".to_string());
          if offset > 0 {
            entries_sql.push(',');
          }
          entries_sql.push_str(&format!(
            "({id}, {root_id}, '{path}', '{path_lower}', '{name}', '{name_lower}', {extension_sql}, {kind}, 8, NULL, 0, 0, 0, 0, 0)",
            path = sql_quote(&spec.path),
            path_lower = sql_quote(&spec.path.to_ascii_lowercase()),
            name = sql_quote(name),
            name_lower = sql_quote(&name.to_ascii_lowercase()),
            kind = if spec.is_dir { 2 } else { 1 },
          ));
          for (term, field, weight) in terms_for(spec) {
            if !first_term {
              terms_sql.push(',');
            }
            first_term = false;
            terms_sql.push_str(&format!(
              "('{}', {id}, {field}, {weight})",
              sql_quote(&term)
            ));
          }
        }
        tx.execute(&entries_sql, ()).await.unwrap();
        tx.execute(&terms_sql, ()).await.unwrap();
      }
      tx.commit().await.unwrap();
    }
  });
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
  let mid = pick(&counts, (rows_u32 / 3300).max(20), (rows_u32 / 1000).max(60));
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
  assert!(rows >= 20_000, "the frequency bands need at least 20,000 rows");
  let base = std::env::var_os("KUNTU_BENCH_DIR")
    .map(PathBuf::from)
    .unwrap_or_else(|| std::env::temp_dir().join("kuntu-bench"));
  fs::create_dir_all(&base).unwrap();
  let root = base.join(format!("kuntu-root-{rows}"));
  fs::create_dir_all(&root).unwrap();
  let root = kuntu_core::normalize_root_path(root);
  let db_path = base.join(format!("realistic-{rows}.sqlite"));
  let queries_path = base.join(format!("realistic-{rows}.queries"));
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
    index.rebuild(&config).unwrap();
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
    // The rebuild indexed the (empty) root directory itself; the fixture owns
    // the whole entries table.
    block_on(async {
      index.conn.execute("DELETE FROM entries", ()).await.unwrap();
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
```

Register the module (insert before the `#[cfg(test)]` line that precedes `mod tests {`):

<!-- apply {"task": 1, "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "#[cfg(test)]\nmod tests {"} -->
```rust
#[cfg(test)]
mod fixture;
```

Append the gate to the end of `mod tests` (before its closing `}`):

<!-- apply {"task": 1, "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
  /// Release-mode latency gate on the generated realistic fixture (see
  /// `fixture.rs`). Rows come from `KUNTU_BENCH_ROWS` (default 100,000; the
  /// owner runs 1,000,000 via `just benchmark-1m`), the cache directory from
  /// `KUNTU_BENCH_DIR`. Each query class runs three times and the best run is
  /// compared with its ceiling, so a cold page cache does not fail the gate.
  ///
  /// Ceilings are the measured warm numbers of the fixed implementation at
  /// 1,000,000 rows (turso 0.7.2, M4 Pro, external disk) with generous headroom
  /// for a slow disk and a loaded machine. They are regression guards, not
  /// targets. The unfixed implementation fails the non-ASCII miss (full terms
  /// scan) and then the broad classes (per-batch root scan) by orders of
  /// magnitude.
  #[test]
  #[ignore = "latency gate on a generated fixture; run in release mode via just benchmark"]
  fn realistic_fixture_latency_gate() {
    let rows: usize = std::env::var("KUNTU_BENCH_ROWS")
      .ok()
      .and_then(|value| value.parse().ok())
      .unwrap_or(100_000);
    let fixture = fixture::open_or_build(rows);
    eprintln!(
      "GATE fixture rows={} bytes={} build_secs={:?}",
      fixture.rows, fixture.db_bytes, fixture.build_secs
    );
    let queries = &fixture.queries;
    // (class, query text, ceiling in milliseconds)
    let classes: Vec<(&str, String, u128)> = vec![
      ("miss", "zzqqx".to_string(), 25),
      ("miss_non_ascii", "zzqqé".to_string(), 25),
      ("rare", queries.rare.clone(), 50),
      ("mid", queries.mid.clone(), 100),
      ("broad", queries.broad.clone(), 400),
      ("prefix3", queries.prefix3.clone(), 400),
      ("mixed", queries.mixed.clone(), 300),
      ("all_rows", queries.all_rows.clone(), 600),
      ("prefix1", queries.prefix1.clone(), 800),
    ];
    for (class, text, ceiling_ms) in classes {
      let query = SearchQuery::new(text).with_limit(20);
      let mut best = std::time::Duration::MAX;
      let mut outcome = None;
      for run in 0..3 {
        let started = std::time::Instant::now();
        let current = fixture
          .index
          .search_with_metrics(&fixture.config, &query)
          .unwrap();
        let elapsed = started.elapsed();
        eprintln!(
          "GATE rows={rows} class={class} run={run} ms={:.1} candidates={} results={} coverage={:?}",
          elapsed.as_secs_f64() * 1000.0,
          current.candidate_count,
          current.results.len(),
          current.coverage
        );
        best = best.min(elapsed);
        outcome = Some(current);
        // A hopeless run is not repeated.
        if elapsed.as_millis() > ceiling_ms * 20 {
          break;
        }
      }
      let outcome = outcome.unwrap();
      match class {
        "miss" | "miss_non_ascii" => {
          assert_eq!(outcome.candidate_count, 0, "{class}");
          assert_eq!(outcome.coverage, CandidateCoverage::Complete, "{class}");
        }
        "rare" => assert!(outcome.candidate_count >= 1, "{class}"),
        "mid" => assert!(outcome.candidate_count >= 10, "{class}"),
        "broad" | "prefix3" => assert!(outcome.candidate_count >= 1_000, "{class}"),
        "mixed" => {
          // Mixed selectivity must be exact: every row with both tokens.
          assert_eq!(outcome.candidate_count, queries.mixed_truth, "{class}");
          assert_eq!(outcome.coverage, CandidateCoverage::Complete, "{class}");
        }
        "all_rows" | "prefix1" => {
          assert_eq!(outcome.coverage, CandidateCoverage::Truncated, "{class}")
        }
        other => unreachable!("{other}"),
      }
      assert!(
        best.as_millis() <= ceiling_ms,
        "{class}: best of 3 runs took {best:?}, ceiling {ceiling_ms} ms"
      );
    }
  }
```

**RED.** Run the gate at 100k rows (the fixture build is part of the first run):

```
cd <repo>
KUNTU_BENCH_ROWS=100000 KUNTU_BENCH_DIR=/Volumes/Portable2TB/tmp/kuntu-bench \
  cargo test --release -p kuntu-index --locked realistic_fixture_latency_gate -- --ignored --nocapture
```

Expected: `GATE fixture rows=100000 ...` and `FIXTURE built ...` lines, `miss` passes, then **the test fails at `miss_non_ascii`**: `miss_non_ascii: best of 3 runs took ~759 ms, ceiling 25 ms`. Record the line. (At 1M rows the same miss costs 15 to 21 s.)

**GREEN for this task** is only that the helper compiles, clippy is clean for it, and the existing non-ignored tests still pass; the gate is meant to stay red until Task 3:

```
cargo test --release -p kuntu-index --locked
cargo clippy -p kuntu-index --all-targets --locked -- -D warnings
```

Expected: 26 passed, 3 ignored (the two old benchmarks and the new gate); clippy clean.

**Commit:**

```
cargo fmt --all
git add crates/kuntu-index/src/fixture.rs crates/kuntu-index/src/lib.rs
git commit -m "test(kuntu-index): realistic Zipf fixture generator and latency gate (red on purpose)"
```


## Task 2: non-ASCII tokens use a range, not `LIKE`

Every non-ASCII token currently becomes `term LIKE 'tok%'`, which scans the terms table (15 to 21 s per miss at 1M rows; 91 to 262 ms at 100k on all engines). A prefix is a range: `term >= tok AND term < tok + U+10FFFF`, a seek on the covering primary-key index. U+10FFFF is a noncharacter, never alphanumeric, so no term contains it and the bound is exact for UTF-8 compared bytewise. ASCII tokens keep the existing incremented-last-byte bound. The clause becomes unqualified (`term`, not `t.term`), which the old stage 1 (still in place) tolerates because its `.replace("t.term", ...)` becomes a no-op and `term` resolves against its only table.

The RED is a runtime assertion failure.


Append the tests to `mod tests` first (they are the RED; run the RED command below before applying the implementation):

<!-- apply {"task": 2, "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
  #[test]
  fn non_ascii_prefixes_use_a_range_not_like() {
    for term in ["é", "café", "计划", "zzqqé"] {
      let (clause, values) = term_prefix_clause(term);
      assert!(!clause.contains("LIKE"), "{term}: {clause}");
      assert_eq!(values.len(), 2, "{term}");
    }
  }

  #[test]
  fn non_ascii_tokens_are_found_by_prefix_and_misses_are_empty() {
    let root = temp_dir("non-ascii-prefix");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("café-menu.txt"), "a\n").unwrap();
    fs::write(root.join("计划书A.md"), "b\n").unwrap();
    fs::write(root.join("plain.txt"), "c\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    for (query, expected) in [
      ("caf", "café-menu.txt"),
      ("café", "café-menu.txt"),
      ("计", "计划书A.md"),
      ("计划", "计划书A.md"),
    ] {
      let outcome = index
        .search_with_metrics(&config, &SearchQuery::new(query))
        .unwrap();
      assert_eq!(outcome.results.len(), 1, "{query}");
      assert!(outcome.results[0].path.ends_with(expected), "{query}");
    }
    let miss = index
      .search_with_metrics(&config, &SearchQuery::new("zzqqé"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(miss.candidate_count, 0);
    assert_eq!(miss.coverage, CandidateCoverage::Complete);
  }
```

Then the implementation. Replace `term_prefix_clause` (from its `fn` line up to, not including, `fn ascii_prefix_upper_bound`) and add the upper-bound helper:

<!-- apply {"task": 2, "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "fn term_prefix_clause(term: &str) -> (String, Vec<Value>) {", "end": "fn ascii_prefix_upper_bound(prefix: &str) -> Option<String> {"} -->
```rust
/// Range predicate for one query token: every term that starts with `term`.
/// Terms are compared bytewise, so the exclusive upper bound is the prefix with
/// its last ASCII byte incremented, or, for anything non-ASCII, the prefix
/// followed by U+10FFFF (a noncharacter that never occurs in a term). A range
/// is a seek on the covering primary-key index; `LIKE` is a scan of the table.
fn term_prefix_clause(term: &str) -> (String, Vec<Value>) {
  (
    "term >= ? AND term < ?".to_string(),
    vec![
      Value::from(term.to_string()),
      Value::from(term_prefix_upper_bound(term)),
    ],
  )
}

fn term_prefix_upper_bound(prefix: &str) -> String {
  ascii_prefix_upper_bound(prefix).unwrap_or_else(|| format!("{prefix}\u{10FFFF}"))
}
```

**RED** (apply the tests only, not the implementation):

```
cargo test --release -p kuntu-index --locked non_ascii
```

Expected: `non_ascii_prefixes_use_a_range_not_like` FAILS (`é: t.term LIKE ?` and `left: 1, right: 2` values); `non_ascii_tokens_are_found_by_prefix_and_misses_are_empty` passes already (behaviour is right, only the plan is wrong), which proves the change is behaviour-preserving.

**GREEN** (apply the implementation):

```
cargo test --release -p kuntu-index --locked non_ascii
cargo test --release -p kuntu-index --locked
KUNTU_BENCH_ROWS=100000 KUNTU_BENCH_DIR=/Volumes/Portable2TB/tmp/kuntu-bench \
  cargo test --release -p kuntu-index --locked realistic_fixture_latency_gate -- --ignored --nocapture
```

Expected: both `non_ascii` tests pass, the whole non-ignored suite passes, and the gate now passes `miss_non_ascii` (best run under 25 ms) and fails further down at `broad` (best of 3 runs 4.45 s against its 400 ms ceiling; the stage-2 whole-root scan, fixed in Task 3). `rare` (about 18 ms) and `mid` (about 64 ms) still pass at 100k rows only because the table is small; at 1M rows they take seconds.

**Commit:**

```
cargo fmt --all
git commit -am "fix(kuntu-index): non-ASCII prefixes use a range instead of LIKE"
```


## Task 3: candidate fetch by rowid, filters in Rust, `SearchOptions::fetch_budget`

**The main fix.** The candidate fetch is replaced by a bare rowid lookup; the removed SQL predicates become `CandidateScope::admit` in Rust (a port, rule for rule). The new `SearchOptions::fetch_budget` bounds the admitted candidates; the loop stops as soon as the budget is reached and reports `Truncated` if ids were left unexamined. Stage 1 is still the old code in this task (the placeholder `let verify_tokens` line exists so stage 2 can already take the driver's verification list in Task 4).

Notes for the worker: `entry_fetch_sql` is pinned by a plan test (`rowid=?`, no `autoindex`). `plant_entry` already exists in `mod tests`; the new tests reuse it. The 60k-row `recall_fixture` helper and `default_budget_bounds_a_broad_single_token` are `#[ignore]` (release-mode, run by `just benchmark`).


**3a.** Tests first (append to `mod tests`). Apply this block alone and run the RED command given after 3f before applying 3b to 3f. The tests use APIs that do not exist yet, so the RED is a compile error:

<!-- apply {"task": 3, "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
  #[test]
  fn entry_fetch_is_a_rowid_seek_not_an_index_scan() {
    let index = KuntuIndex::open_memory().unwrap();
    let sql = format!("EXPLAIN QUERY PLAN {}", entry_fetch_sql(3));
    let details = block_on(async {
      let mut rows = index
        .conn
        .query(
          &sql,
          params_from_iter(vec![Value::from(1i64), Value::from(2i64), Value::from(3i64)]),
        )
        .await
        .unwrap();
      let mut details = Vec::new();
      while let Some(row) = rows.next().await.unwrap() {
        details.push(row.get::<String>(3).unwrap());
      }
      details
    });
    let plan = details.join(" | ");
    assert!(plan.contains("rowid=?"), "plan: {plan}");
    assert!(!plan.contains("autoindex"), "plan: {plan}");
  }

  #[test]
  fn default_fetch_budget_is_five_thousand() {
    assert_eq!(DEFAULT_FETCH_BUDGET, 5_000);
    assert_eq!(SearchOptions::default().fetch_budget, 5_000);
  }

  #[test]
  fn fetch_budget_counts_admitted_candidates_not_ids() {
    // 120 directories hold the lowest ids, then 30 files. A files-only search
    // with a budget of 10 must still return files: the budget counts admitted
    // rows, it is not a window over raw ids.
    let root = temp_dir("budget-window");
    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    for number in 0..120 {
      plant_entry(
        &index,
        &root,
        &format!("plan-dir-{number:03}"),
        2,
        (false, false, false),
        &["plan"],
      );
    }
    for number in 0..30 {
      plant_entry(
        &index,
        &root,
        &format!("plan-file-{number:02}.md"),
        1,
        (false, false, false),
        &["plan"],
      );
    }
    let files_only = SearchQuery {
      include_directories: false,
      ..SearchQuery::new("plan").with_limit(100)
    };

    let tight = index
      .search_with_options(&config, &files_only, SearchOptions { fetch_budget: 10 })
      .unwrap();
    assert_eq!(tight.candidate_count, 10);
    assert_eq!(tight.results.len(), 10);
    assert!(tight.results.iter().all(|r| r.kind == EntryKind::File));
    assert_eq!(tight.coverage, CandidateCoverage::Truncated);

    let exact = index
      .search_with_options(&config, &files_only, SearchOptions { fetch_budget: 30 })
      .unwrap();
    assert_eq!(exact.candidate_count, 30);
    assert_eq!(exact.coverage, CandidateCoverage::Complete);

    let roomy = index.search_with_metrics(&config, &files_only).unwrap();
    assert_eq!(roomy.candidate_count, 30);
    assert_eq!(roomy.coverage, CandidateCoverage::Complete);
    remove_dir_all_if_exists(&root).unwrap();
  }

  #[test]
  fn policy_filters_apply_before_the_budget_not_after() {
    // Forbidden rows (hidden, ignored, sensitive) take the lowest ids. With a
    // budget of 2 the two allowed files behind them must still be returned.
    let root = temp_dir("budget-policy");
    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    for number in 0..6 {
      let flags = match number % 3 {
        0 => (true, false, false),
        1 => (false, true, false),
        _ => (false, false, true),
      };
      plant_entry(
        &index,
        &root,
        &format!("hidden-{number}/plan-secret-{number}.md"),
        1,
        flags,
        &["plan"],
      );
    }
    for number in 0..2 {
      plant_entry(
        &index,
        &root,
        &format!("docs/plan-ok-{number}.md"),
        1,
        (false, false, false),
        &["plan"],
      );
    }
    let outcome = index
      .search_with_options(
        &config,
        &SearchQuery::new("plan").with_limit(10),
        SearchOptions { fetch_budget: 2 },
      )
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 2);
    assert!(outcome
      .results
      .iter()
      .all(|r| r.path.to_string_lossy().contains("plan-ok-")));
  }

  #[test]
  fn fetch_budget_is_clamped_to_a_sane_range() {
    let root = temp_dir("budget-clamp");
    fs::create_dir_all(&root).unwrap();
    for name in ["plan-a.md", "plan-b.md", "plan-c.md"] {
      fs::write(root.join(name), "x\n").unwrap();
    }
    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let query = SearchQuery::new("plan").with_limit(10);

    let zero = index
      .search_with_options(&config, &query, SearchOptions { fetch_budget: 0 })
      .unwrap();
    assert_eq!(zero.candidate_count, 1);
    assert_eq!(zero.coverage, CandidateCoverage::Truncated);

    let huge = index
      .search_with_options(
        &config,
        &query,
        SearchOptions {
          fetch_budget: usize::MAX,
        },
      )
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();
    assert_eq!(huge.candidate_count, 3);
    assert_eq!(huge.coverage, CandidateCoverage::Complete);
  }

  /// 60,000 file entries named `common-N.md`; the entries at `rare_positions`
  /// are named `rareword-common-N.md`. File-backed, planted in bulk.
  struct RecallFixture {
    index: KuntuIndex,
    config: SearchConfig,
    root: PathBuf,
    db_dir: PathBuf,
  }

  impl RecallFixture {
    fn cleanup(self) {
      let _ = remove_dir_all_if_exists(&self.db_dir);
      let _ = remove_dir_all_if_exists(&self.root);
    }
  }

  fn recall_fixture(name: &str, rare_positions: &[usize]) -> RecallFixture {
    let root = temp_dir(name);
    fs::create_dir_all(&root).unwrap();
    let root = kuntu_core::normalize_root_path(root);
    let db_dir = temp_dir(&format!("{name}-db"));
    fs::create_dir_all(&db_dir).unwrap();
    let mut index = KuntuIndex::open(db_dir.join("recall.sqlite")).unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let root_id: i64 = block_on(async {
      let mut rows = index
        .conn
        .query(
          "SELECT id FROM roots WHERE path = ?1",
          params![root.to_string_lossy()],
        )
        .await
        .unwrap();
      rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    });
    block_on(async {
      index.conn.execute("DELETE FROM entries", ()).await.unwrap();
    });
    let rare: HashSet<usize> = rare_positions.iter().copied().collect();
    let rows: Vec<fixture::RowSpec> = (0..60_000usize)
      .map(|number| {
        let file_name = if rare.contains(&number) {
          format!("rareword-common-{number}.md")
        } else {
          format!("common-{number}.md")
        };
        fixture::RowSpec {
          path: format!("{}/{file_name}", root.to_string_lossy()),
          is_dir: false,
        }
      })
      .collect();
    fixture::insert_rows(&mut index, root_id, &rows, 1);
    RecallFixture {
      index,
      config,
      root,
      db_dir,
    }
  }

  #[test]
  #[ignore = "60k-row fixture; run in release mode via just benchmark"]
  fn default_budget_bounds_a_broad_single_token() {
    let fixture = recall_fixture("budget-broad", &[]);
    let query = SearchQuery::new("common").with_limit(50);

    let outcome = fixture
      .index
      .search_with_metrics(&fixture.config, &query)
      .unwrap();
    assert_eq!(outcome.candidate_count, DEFAULT_FETCH_BUDGET);
    assert_eq!(outcome.coverage, CandidateCoverage::Truncated);
    assert_eq!(outcome.results.len(), 50);
    assert!(outcome.truncated);

    let wide = fixture
      .index
      .search_with_options(
        &fixture.config,
        &query,
        SearchOptions {
          fetch_budget: MAX_KFS_CANDIDATE_ROWS,
        },
      )
      .unwrap();
    assert_eq!(wide.candidate_count, MAX_KFS_CANDIDATE_ROWS);
    assert_eq!(wide.coverage, CandidateCoverage::Truncated);
    fixture.cleanup();
  }
```

**3b.** Public types and constants. Insert before `impl MetadataIndex for KuntuIndex {`:

<!-- apply {"task": 3, "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "impl MetadataIndex for KuntuIndex {"} -->
```rust
/// Default number of candidates one search loads and ranks. Candidates are
/// counted after scope, kind, extension and policy filtering, so rows the
/// caller may not see never consume the budget. Reaching it reports
/// [`CandidateCoverage::Truncated`].
pub const DEFAULT_FETCH_BUDGET: usize = 5_000;

/// Ids per `IN (...)` statement when loading candidate rows.
const FETCH_CHUNK_IDS: usize = 1_000;

/// Per-search tuning. `search_with_metrics` uses the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
  /// Maximum admitted candidates to load and rank. Clamped to
  /// `1..=MAX_KFS_CANDIDATE_ROWS`.
  pub fetch_budget: usize,
}

impl Default for SearchOptions {
  fn default() -> Self {
    Self {
      fetch_budget: DEFAULT_FETCH_BUDGET,
    }
  }
}
```

**3c.** Split the search entry point. Replace from the `pub fn search_with_metrics(` line up to, not including, the line `      let empty = IndexedSearchOutcome {`:

<!-- apply {"task": 3, "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "  pub fn search_with_metrics(", "end": "      let empty = IndexedSearchOutcome {"} -->
```rust
  pub fn search_with_metrics(
    &self,
    config: &SearchConfig,
    query: &SearchQuery,
  ) -> Result<IndexedSearchOutcome> {
    self.search_with_options(config, query, SearchOptions::default())
  }

  /// Search with explicit options. Scope, kind, extension and policy filters
  /// apply to every candidate before ranking and before the fetch budget and
  /// the result window, exactly as in `search_with_metrics`.
  pub fn search_with_options(
    &self,
    config: &SearchConfig,
    query: &SearchQuery,
    options: SearchOptions,
  ) -> Result<IndexedSearchOutcome> {
    let budget = options.fetch_budget.clamp(1, MAX_KFS_CANDIDATE_ROWS);
    block_on(async {
```

**3d.** Stage 2. Replace from the line `      let mut clauses = vec!["entries_main.deleted = 0".to_string()];` up to, not including, `      let candidate_count = candidates.len();` (this removes the old predicate list and the JOIN fetch):

<!-- apply {"task": 3, "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "      let mut clauses = vec![\"entries_main.deleted = 0\".to_string()];", "end": "      let candidate_count = candidates.len();"} -->
```rust
      let verify_tokens: Vec<String> = Vec::new();

      // Stage 2: load the candidate rows by id and filter in Rust.
      //
      // The fetch statement carries no predicate at all. turso's planner turns
      // any indexed predicate beside the id list (root_id, extension) into a
      // scan of that index, which costs a whole-root scan per batch; the bare
      // `id IN (...)` is a rowid seek. Scope, kind, extension and policy are
      // therefore applied to each fetched row before it can become a
      // candidate, so nothing outside the configured scope or policy is ever
      // ranked, and the budget counts only admitted rows.
      let scope = CandidateScope::load(&self.conn, &root_ids, query).await?;
      let mut candidates: Vec<CandidateRow> = Vec::new();
      let mut examined = 0usize;
      'chunks: for chunk in id_list.chunks(FETCH_CHUNK_IDS) {
        let mut admitted: HashMap<i64, CandidateRow> = HashMap::new();
        for entry in fetch_entries_by_id(&self.conn, chunk).await? {
          if !verify_tokens.is_empty() && !path_has_prefix_tokens(&entry.path, &verify_tokens) {
            continue;
          }
          let id = entry.id;
          if let Some(candidate) = scope.admit(query, entry) {
            admitted.insert(id, candidate);
          }
        }
        for id in chunk {
          examined += 1;
          if let Some(candidate) = admitted.remove(id) {
            candidates.push(candidate);
            if candidates.len() == budget {
              break 'chunks;
            }
          }
        }
      }
      // The budget stopped the search before every id was examined.
      let budget_truncated = candidates.len() == budget && examined < id_list.len();
      let any_token_saturated = any_token_saturated || budget_truncated;
```

**3e.** Helpers. Insert before the `#[derive(Debug)]` line of `struct CandidateRow`:

<!-- apply {"task": 3, "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "#[derive(Debug)]\nstruct CandidateRow {"} -->
```rust
/// One `entries` row as stored, fetched by id.
struct FetchedEntry {
  id: i64,
  root_id: i64,
  path: PathBuf,
  kind: EntryKind,
  size: Option<u64>,
  mtime: Option<u64>,
  extension: Option<String>,
  hidden: bool,
  ignored: bool,
  sensitive: bool,
  deleted: bool,
}

/// The candidate fetch. Keep it free of predicates: see the note at stage 2 of
/// `search_with_options` and the plan test that pins this statement.
fn entry_fetch_sql(id_count: usize) -> String {
  format!(
    "SELECT id, root_id, path, kind, size, mtime, extension, hidden, ignored, sensitive, deleted
       FROM entries WHERE id IN ({})",
    placeholders(id_count)
  )
}

async fn fetch_entries_by_id(conn: &turso::Connection, ids: &[i64]) -> Result<Vec<FetchedEntry>> {
  let values: Vec<Value> = ids.iter().copied().map(Value::from).collect();
  let mut rows = conn
    .query(&entry_fetch_sql(ids.len()), params_from_iter(values))
    .await?;
  let mut fetched = Vec::with_capacity(ids.len());
  while let Some(row) = rows.next().await? {
    let mtime: Option<i64> = row.get(5)?;
    fetched.push(FetchedEntry {
      id: row.get::<i64>(0)?,
      root_id: row.get::<i64>(1)?,
      path: PathBuf::from(row.get::<String>(2)?),
      kind: int_to_kind(row.get::<i64>(3)?),
      size: u64::try_from(row.get::<i64>(4).unwrap_or(-1)).ok(),
      mtime: u64::try_from(mtime.unwrap_or(-1)).ok(),
      extension: row.get(6)?,
      hidden: row.get::<i64>(7)? != 0,
      ignored: row.get::<i64>(8)? != 0,
      sensitive: row.get::<i64>(9)? != 0,
      deleted: row.get::<i64>(10)? != 0,
    });
  }
  Ok(fetched)
}

/// Everything a fetched row is checked against before it can be ranked.
struct CandidateScope {
  allowed_roots: HashSet<i64>,
  /// root id -> (priority, include_hidden, include_ignored)
  root_meta: HashMap<i64, (i32, bool, bool)>,
  extensions: Vec<String>,
}

impl CandidateScope {
  async fn load(conn: &turso::Connection, root_ids: &[i64], query: &SearchQuery) -> Result<Self> {
    let mut root_meta = HashMap::new();
    let mut rows = conn
      .query(
        "SELECT id, priority, include_hidden, include_ignored FROM roots",
        (),
      )
      .await?;
    while let Some(row) = rows.next().await? {
      root_meta.insert(
        row.get::<i64>(0)?,
        (
          row.get::<i32>(1)?,
          row.get::<i64>(2)? != 0,
          row.get::<i64>(3)? != 0,
        ),
      );
    }
    Ok(Self {
      allowed_roots: root_ids.iter().copied().collect(),
      root_meta,
      extensions: query
        .extensions
        .iter()
        .map(|extension| extension.trim_start_matches('.').to_ascii_lowercase())
        .collect(),
    })
  }

  /// The v0.4.0 admission rules, unchanged: a row is a candidate only if it is
  /// live, not sensitive, inside a configured root, of a requested kind, has a
  /// requested extension, and is either clean or allowed by the query flag or
  /// by its own root's allowance.
  fn admit(&self, query: &SearchQuery, entry: FetchedEntry) -> Option<CandidateRow> {
    if entry.deleted || entry.sensitive || !self.allowed_roots.contains(&entry.root_id) {
      return None;
    }
    let kind_allowed = match entry.kind {
      EntryKind::File => query.include_files,
      EntryKind::Directory => query.include_directories,
      EntryKind::Other => query.include_files && query.include_directories,
    };
    if !kind_allowed {
      return None;
    }
    let (priority, root_hidden, root_ignored) = self
      .root_meta
      .get(&entry.root_id)
      .copied()
      .unwrap_or((0, false, false));
    if entry.hidden && !(query.include_hidden || root_hidden) {
      return None;
    }
    if entry.ignored && !(query.include_ignored || root_ignored) {
      return None;
    }
    if !self.extensions.is_empty()
      && !entry
        .extension
        .as_ref()
        .is_some_and(|extension| self.extensions.contains(extension))
    {
      return None;
    }
    Some(CandidateRow {
      path: entry.path,
      kind: entry.kind,
      root_priority: priority,
      byte_size: entry.size,
      modified_unix_seconds: entry.mtime,
    })
  }
}

/// True when every token is a prefix of some token of the path. The terms
/// table holds exactly the tokens of the path components, so this is the same
/// predicate the index evaluates per token, applied to one row.
fn path_has_prefix_tokens(path: &Path, tokens: &[String]) -> bool {
  let path_tokens = tokenize(&path.to_string_lossy());
  tokens.iter().all(|token| {
    path_tokens
      .iter()
      .any(|candidate| candidate.starts_with(token.as_str()))
  })
}
```

**3f.** The single-term benchmark asserts that a broad query yields exactly `MAX_KFS_CANDIDATE_ROWS` candidates; with the new default budget it must ask for the wide budget explicitly. In `run_bounded_fixture`, replace the `.search_with_metrics(&config, &SearchQuery::new("batchfile").with_limit(100))` line (the call that produces `broad`):

<!-- apply {"task": 3, "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "      .search_with_metrics(&config, &SearchQuery::new(\"batchfile\").with_limit(100))\n", "end": "      .unwrap();\n    let broad_elapsed"} -->
```rust
      .search_with_options(
        &config,
        &SearchQuery::new("batchfile").with_limit(100),
        SearchOptions {
          fetch_budget: MAX_KFS_CANDIDATE_ROWS,
        },
      )
```

**RED** (after 3a only, before 3b to 3f):

```
cargo test --release -p kuntu-index --locked
```

Expected: **compile errors** `cannot find function entry_fetch_sql`, `cannot find value DEFAULT_FETCH_BUDGET`, `cannot find type SearchOptions`, `no method named search_with_options`. (There is no behavioural RED for a new API; the behavioural proof is the gate, which stays red at its broad classes: before this task it takes seconds per candidate batch.)

**GREEN** (after 3b to 3f):

```
cargo test --release -p kuntu-index --locked
cargo test --release -p kuntu-index --locked -- --ignored --nocapture --test-threads=1 default_budget_bounds_a_broad_single_token broad_prefix_and_miss_stay_bounded_at_60k_rows
KUNTU_BENCH_ROWS=100000 KUNTU_BENCH_DIR=/Volumes/Portable2TB/tmp/kuntu-bench \
  cargo test --release -p kuntu-index --locked realistic_fixture_latency_gate -- --ignored --nocapture
```

Expected: the non-ignored suite passes (38 tests, including the pre-existing v0.4.0 policy, scope, kind-window, extension and tie-break tests, which prove the port); the plan test passes; the two ignored fixtures pass (the 60k single-term benchmark took 31 s per broad query before and now finishes in about 2 s); the 100k gate passes all nine classes (`broad` 4.4 s becomes about 55 ms).

The one defect this task deliberately leaves is stage 1. It shows once the fixture is large enough for the common extension token to exceed the 50,000-id work bound. Run the gate at 300,000 rows (the fixture build takes about 5 minutes and about 3 GB with turso's write-ahead log; later runs reuse it):

```
KUNTU_BENCH_ROWS=300000 KUNTU_BENCH_DIR=/Volumes/Portable2TB/tmp/kuntu-bench \
  cargo test --release -p kuntu-index --locked realistic_fixture_latency_gate -- --ignored --nocapture
```

Expected: every class up to `prefix3` passes, then **`mixed` fails**: `assertion left == right failed: mixed`, `left: 145`, `right: 169`, with `coverage=Truncated` in the `GATE` line (the same bug as `tujoko ts`: the selective token's rows beyond the common token's capped list are lost). Task 4 closes it.

**Commit:**

```
cargo fmt --all
git commit -am "fix(kuntu-index): fetch candidates by rowid, filter in Rust, add fetch budget"
```


## Task 4: stage 1 range scans and the rarest-token driver (exact multi-token recall)

Two problems remain in stage 1: the per-token statement still has `DISTINCT` and `ORDER BY entry_id` (turso then picks the non-covering `idx_terms_term` plus a sorter; 413 ms for 1,800 rows and 1.2 s for 50k rows at 100k entries, against 0.6 ms and 8 ms for a plain range), and intersecting per-token lists capped at 50,000 loses rows when one token is selective and another is common (`tujoko ts`: 8 returned of 33).

New stage 1: load each unique token's posting rows with a plain range (`SELECT entry_id, field FROM terms WHERE term >= ? AND term < ? LIMIT ?`), first with a small probe. If any token's whole list fits, the **smallest complete list drives the search**: its ids are fetched and every other token is verified against the fetched row's path (`path_has_prefix_tokens`), which is exactly the predicate the terms table encodes, so the result is exact and `Complete`. If every token is too broad to probe, all lists are loaded up to the work bound; a complete one still drives, otherwise the capped lists are intersected and the result is reported `Truncated`. Ids are ordered file-name hits first (field 1), then by id, so a binding budget keeps the strongest rows deterministically.


**4a.** Behaviour tests first (append to `mod tests`). They compile against Task 3's code, so the RED is a real assertion failure:

<!-- apply {"task": 4, "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
  #[test]
  fn every_query_token_must_match_and_tokens_match_by_prefix() {
    let root = temp_dir("multi-token");
    fs::create_dir_all(root.join("alphabet")).unwrap();
    fs::write(root.join("alphabet/beta-notes.md"), "a\n").unwrap();
    fs::write(root.join("alphabet/gamma-notes.md"), "b\n").unwrap();
    fs::write(root.join("other.md"), "c\n").unwrap();

    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    let both = index
      .search_with_metrics(&config, &SearchQuery::new("alph bet"))
      .unwrap();
    let one_missing = index
      .search_with_metrics(&config, &SearchQuery::new("alph delta"))
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(both.candidate_count, 1);
    assert!(both.results[0].path.ends_with("alphabet/beta-notes.md"));
    assert_eq!(both.coverage, CandidateCoverage::Complete);
    assert_eq!(one_missing.candidate_count, 0);
    assert_eq!(one_missing.coverage, CandidateCoverage::Complete);
  }

  /// Plant `relative` with `token` recorded as a directory-component hit only
  /// (field 3), as the indexer does for a token that appears only in the path.
  fn plant_path_only_entry(index: &KuntuIndex, root: &Path, relative: &str, token: &str) {
    plant_entry(index, root, relative, 1, (false, false, false), &[token]);
    let full = kuntu_core::normalize_root_path(root.to_path_buf()).join(relative);
    block_on(async {
      index
        .conn
        .execute(
          "UPDATE terms SET field = 3 WHERE entry_id = (SELECT id FROM entries WHERE path = ?1)",
          params![full.to_string_lossy()],
        )
        .await
        .unwrap();
    });
  }

  #[test]
  fn file_name_hits_are_fetched_before_path_only_hits_when_the_budget_binds() {
    // Five path-only matches take the lowest ids, five file-name matches come
    // after them. With a budget of 5 the file-name matches must win.
    let root = temp_dir("name-first");
    let mut index = KuntuIndex::open_memory().unwrap();
    let config = SearchConfig {
      roots: vec![SearchRoot::new(&root)],
    };
    index.rebuild(&config).unwrap();
    for number in 0..5 {
      plant_path_only_entry(
        &index,
        &root,
        &format!("tokenword-dir/aaa-{number}.md"),
        "tokenword",
      );
    }
    for number in 0..5 {
      plant_entry(
        &index,
        &root,
        &format!("zzz-tokenword-{number}.md"),
        1,
        (false, false, false),
        &["tokenword"],
      );
    }
    let outcome = index
      .search_with_options(
        &config,
        &SearchQuery::new("tokenword").with_limit(20),
        SearchOptions { fetch_budget: 5 },
      )
      .unwrap();
    remove_dir_all_if_exists(&root).unwrap();

    assert_eq!(outcome.candidate_count, 5);
    assert_eq!(outcome.coverage, CandidateCoverage::Truncated);
    assert!(outcome.results.iter().all(|result| result
      .path
      .file_name()
      .is_some_and(|name| name.to_string_lossy().starts_with("zzz-"))));
  }

  /// The multi-token recall bug: a selective token plus a common one. 60,000
  /// entries carry `common`; 33 also carry `rareword`. Eight of them have ids
  /// below the 50,000-id work bound, 25 above it. Capping each token's list
  /// and intersecting returns only the 8; driving from the rare token returns
  /// all 33 and stays `Complete`.
  #[test]
  #[ignore = "60k-row fixture; run in release mode via just benchmark"]
  fn rarest_token_drives_the_search_and_recall_is_exact() {
    let rare_positions: Vec<usize> = (0..8).chain(55_000..55_025).collect();
    let fixture = recall_fixture("recall", &rare_positions);
    for text in ["rareword common", "common rareword", "rare common"] {
      let outcome = fixture
        .index
        .search_with_metrics(&fixture.config, &SearchQuery::new(text).with_limit(1_000))
        .unwrap();
      assert_eq!(outcome.candidate_count, 33, "{text}");
      assert_eq!(outcome.coverage, CandidateCoverage::Complete, "{text}");
      assert_eq!(outcome.results.len(), 33, "{text}");
    }
    fixture.cleanup();
  }
```

**RED** (4a only; Task 3's code):

```
cargo test --release -p kuntu-index --locked file_name_hits_are_fetched_before -- --nocapture
cargo test --release -p kuntu-index --locked every_query_token_must_match
cargo test --release -p kuntu-index --locked rarest_token_drives_the_search -- --ignored --nocapture
```

Expected: `file_name_hits_are_fetched_before_path_only_hits_when_the_budget_binds` FAILS (a result file name starts with `aaa-`: path-only hits take the lowest ids); `every_query_token_must_match_and_tokens_match_by_prefix` passes (guard for the verification semantics); `rarest_token_drives_the_search_and_recall_is_exact` FAILS with `left: 8, right: 33` (the intersection bug, `candidate_count` 8 and `Truncated`).


**4b.** Helpers (insert before the `#[derive(Debug)]` line of `struct CandidateRow`):

<!-- apply {"task": 4, "file": "crates/kuntu-index/src/lib.rs", "mode": "insert-before", "anchor": "#[derive(Debug)]\nstruct CandidateRow {"} -->
```rust
/// Rows read from the terms table to decide whether a token's whole posting
/// list is small enough to drive a search.
const DRIVER_PROBE_ROWS: usize = 1_000;

/// One token's posting list: entry id -> whether the token hit the file name
/// (field 1) of that entry.
struct TokenPostings {
  ids: std::collections::BTreeMap<i64, bool>,
  /// False when a row or distinct-id limit cut the list short.
  complete: bool,
}

/// Load the posting rows of every term that starts with `term`, stopping after
/// `row_limit` rows or `distinct_cap` distinct ids. The statement is a plain
/// range over the covering primary-key index: no `DISTINCT`, no `ORDER BY`
/// (either makes turso choose the non-covering `idx_terms_term` and a sorter).
async fn load_postings(
  conn: &turso::Connection,
  term: &str,
  row_limit: usize,
  distinct_cap: usize,
) -> Result<TokenPostings> {
  let (clause, mut values) = term_prefix_clause(term);
  let sql = format!("SELECT entry_id, field FROM terms WHERE {clause} LIMIT ?");
  values.push(Value::from(i64::try_from(row_limit).unwrap_or(i64::MAX - 1) + 1));
  let mut rows = conn.query(&sql, params_from_iter(values)).await?;
  let mut ids = std::collections::BTreeMap::new();
  let mut seen_rows = 0usize;
  let mut complete = true;
  while let Some(row) = rows.next().await? {
    if seen_rows == row_limit {
      complete = false;
      break;
    }
    seen_rows += 1;
    let id = row.get::<i64>(0)?;
    let name_hit = row.get::<i64>(1)? == 1;
    if !ids.contains_key(&id) && ids.len() == distinct_cap {
      complete = false;
      break;
    }
    let entry = ids.entry(id).or_insert(false);
    *entry = *entry || name_hit;
  }
  Ok(TokenPostings { ids, complete })
}

/// Position of the complete list with the fewest entries.
fn smallest_complete(lists: &[(String, TokenPostings)]) -> Option<usize> {
  lists
    .iter()
    .enumerate()
    .filter(|(_, (_, postings))| postings.complete)
    .min_by_key(|(_, (_, postings))| postings.ids.len())
    .map(|(position, _)| position)
}

/// Ids present in every list, file-name hits first (by how many tokens hit
/// the file name), then by id, so a truncated search keeps the strongest rows
/// and stays deterministic.
fn rank_ids(lists: &[&std::collections::BTreeMap<i64, bool>]) -> Vec<i64> {
  let Some((first, rest)) = lists.split_first() else {
    return Vec::new();
  };
  let mut ranked: Vec<(usize, i64)> = Vec::new();
  for (&id, &name_hit) in first.iter() {
    let mut name_hits = usize::from(name_hit);
    let mut in_every_list = true;
    for other in rest {
      match other.get(&id) {
        Some(&hit) => name_hits += usize::from(hit),
        None => {
          in_every_list = false;
          break;
        }
      }
    }
    if in_every_list {
      ranked.push((name_hits, id));
    }
  }
  ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
  ranked.into_iter().map(|(_, id)| id).collect()
}
```

**4c.** Unit tests for the pure helpers (append to `mod tests`):

<!-- apply {"task": 4, "file": "crates/kuntu-index/src/lib.rs", "mode": "append-to-tests-module"} -->
```rust
  #[test]
  fn rank_ids_intersects_and_puts_file_name_hits_first() {
    let first = std::collections::BTreeMap::from([(1, false), (2, true), (3, true), (4, false)]);
    let second = std::collections::BTreeMap::from([(2, false), (3, true), (4, true), (9, true)]);
    // Present in both: 2, 3, 4. Name hits: 3 -> 2, 2 -> 1, 4 -> 1.
    assert_eq!(rank_ids(&[&first, &second]), vec![3, 2, 4]);
    assert_eq!(rank_ids(&[&first]), vec![2, 3, 1, 4]);
    assert!(rank_ids(&[]).is_empty());
  }

  #[test]
  fn smallest_complete_ignores_truncated_lists() {
    let list = |ids: &[i64], complete: bool| TokenPostings {
      ids: ids.iter().map(|id| (*id, false)).collect(),
      complete,
    };
    let lists = vec![
      ("big".to_string(), list(&[1, 2, 3, 4], true)),
      ("cut".to_string(), list(&[1], false)),
      ("small".to_string(), list(&[1, 2], true)),
    ];
    assert_eq!(smallest_complete(&lists), Some(2));
    assert_eq!(smallest_complete(&lists[1..2]), None);
  }

  #[test]
  fn path_tokens_are_matched_by_prefix_case_insensitively() {
    let path = Path::new("/home/me/Alphabet/beta-notes.md");
    assert!(path_has_prefix_tokens(
      path,
      &["alph".to_string(), "bet".to_string()]
    ));
    assert!(!path_has_prefix_tokens(
      path,
      &["alph".to_string(), "gamma".to_string()]
    ));
    assert!(path_has_prefix_tokens(path, &[]));
  }
```

**4d.** Stage 1. Replace from the comment line `      // Candidate selection runs in two stages.` up to, not including, the comment line `      // Stage 2: load the candidate rows by id and filter in Rust.` This removes the old per-token `SELECT DISTINCT ... ORDER BY` loop, the id-set intersection, and the placeholder `let verify_tokens` line that Task 3 left behind:

<!-- apply {"task": 4, "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "      // Candidate selection runs in two stages.", "end": "      // Stage 2: load the candidate rows by id and filter in Rust."} -->
```rust
      // Stage 1: posting lists, one range seek per unique token.
      //
      // A token's list is cheap to load, but two broad tokens cannot be
      // intersected by capping each list (a selective token plus a common one
      // would silently lose rows: the common list is cut at the work bound and
      // most of the selective token's rows fall outside it). Instead the
      // smallest *complete* list drives the search: its entries are fetched
      // and every other token is verified against the row's own path, which is
      // exactly the predicate the terms table encodes. Only when every token
      // is too broad to load completely are the capped lists intersected, and
      // the result is then reported as truncated.
      let unique_terms: std::collections::BTreeSet<String> = query_terms.into_iter().collect();

      let mut lists: Vec<(String, TokenPostings)> = Vec::new();
      for term in &unique_terms {
        let postings = load_postings(&self.conn, term, DRIVER_PROBE_ROWS, usize::MAX).await?;
        if postings.complete && postings.ids.is_empty() {
          // A token that matches nothing ends the search: empty and complete.
          return Ok(empty);
        }
        lists.push((term.clone(), postings));
      }
      let mut driver = smallest_complete(&lists);
      if driver.is_none() {
        lists.clear();
        for term in &unique_terms {
          let postings = load_postings(
            &self.conn,
            term,
            MAX_KFS_CANDIDATE_ROWS * 3,
            MAX_KFS_CANDIDATE_ROWS,
          )
          .await?;
          lists.push((term.clone(), postings));
        }
        driver = smallest_complete(&lists);
      }
      let (id_list, verify_tokens, any_token_saturated): (Vec<i64>, Vec<String>, bool) = match driver
      {
        Some(position) => {
          let verify_tokens = lists
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != position)
            .map(|(_, (term, _))| term.clone())
            .collect();
          (rank_ids(&[&lists[position].1.ids]), verify_tokens, false)
        }
        None => {
          let id_maps: Vec<&std::collections::BTreeMap<i64, bool>> =
            lists.iter().map(|(_, postings)| &postings.ids).collect();
          (rank_ids(&id_maps), Vec::new(), true)
        }
      };
```

**GREEN:**

```
cargo test --release -p kuntu-index --locked
cargo test --release -p kuntu-index --locked -- --ignored --nocapture --test-threads=1 default_budget_bounds_a_broad_single_token rarest_token_drives_the_search broad_prefix_and_miss_stay_bounded_at_60k_rows
cargo clippy -p kuntu-index --all-targets --locked -- -D warnings
for rows in 100000 300000; do
  KUNTU_BENCH_ROWS=$rows KUNTU_BENCH_DIR=/Volumes/Portable2TB/tmp/kuntu-bench \
    cargo test --release -p kuntu-index --locked realistic_fixture_latency_gate -- --ignored --nocapture
done
```

Expected: everything passes, including the pure-helper unit tests (38 + 5 ignored in the suite: 43 tests listed), the name-first test, and the whole pre-existing suite; the recall test returns 33 of 33 with `Complete`; the gate passes all nine classes at 100k **and at 300k** (`mixed` returns the exact 169 candidates, `Complete`, in about 37 ms warm). Warm best-of-3 at 300k rows: `broad` and `prefix3` about 29 ms, `all_rows` about 29 ms, `prefix1` about 43 ms. The `run=0` lines are cold-cache runs on an external disk (up to 1.7 s at 300k rows) and are not gated.

**Commit:**

```
cargo fmt --all
git commit -am "fix(kuntu-index): range scans on the covering index, rarest-token driver with exact verification"
```


## Task 5: benchmark recipes, stale comments and the release note

No behaviour change. The `justfile` gains a recipe for the 1M gate and a single-threaded `benchmark` (timings must not compete), the stale "KNOWN ENGINE GAP" comment on the old 1M test is corrected, and the README gets one `Revisions` row.


Replace the `benchmark` recipe in `justfile` (the whole file is rewritten):

<!-- apply {"task": 5, "file": "justfile", "mode": "create"} -->
```make

# Release-mode benchmarks (ignored tests; a debug turso build is far too slow).
# Runs the single-term bound tests, the 60k recall/budget fixtures, and the
# realistic latency gate at KUNTU_BENCH_ROWS rows (default 100,000).
# Cached fixtures live in KUNTU_BENCH_DIR (default <tmp>/kuntu-bench).
benchmark:
    cargo test --release -p kuntu-index --locked -- --ignored --nocapture --test-threads=1

# The 1,000,000-row latency gate on the generated realistic fixture. The first
# run builds the fixture (minutes, and about 1.5 GB of disk); later runs reuse it.
benchmark-1m:
    KUNTU_BENCH_ROWS=1000000 cargo test --release -p kuntu-index --locked realistic_fixture_latency_gate -- --ignored --nocapture
```

Correct the stale doc comment on the old 1M test. In `crates/kuntu-index/src/lib.rs`, replace from the line `  /// The plan's one-million-row benchmark. Debug-profile turso inserts grow` up to, not including, `  #[test]` that precedes `#[ignore = "one-million-row benchmark; run in release mode"]`:

<!-- apply {"task": 5, "file": "crates/kuntu-index/src/lib.rs", "mode": "replace-range", "start": "  /// The plan's one-million-row benchmark. Debug-profile turso inserts grow", "end": "  #[test]\n  #[ignore = \"one-million-row benchmark; run in release mode\"]"} -->
```rust
  /// The single-term one-million-row benchmark: one posting list of 1,000,000
  /// rows. The miss is a seek (about 1 ms) and the wide-budget broad query
  /// loads exactly `MAX_KFS_CANDIDATE_ROWS` candidates. This fixture is
  /// pathological on purpose; the realistic 1M benchmark is
  /// `realistic_fixture_latency_gate` (`just benchmark-1m`). Debug-profile
  /// turso inserts grow superlinearly past ~200k rows, so this runs ignored;
  /// measure with
  /// `cargo test --release -p kuntu-index one_million -- --ignored --nocapture`.
  ///
  /// History: before v0.5.0 this test failed its broad ceiling (1,005 s) because
  /// the candidate fetch scanned the whole root once per batch. The README's
  /// earlier note blaming the terms range predicate (a "7.5 s miss") was wrong:
  /// the range was always a seek.
```

Add the release row to the `Revisions` table in `README.md`, directly above the `v0.4.0` row (insert before the text `| `v0.4.0` | Search hardening`); the row is one line:

<!-- apply {"task": 5, "file": "README.md", "mode": "insert-before", "gap": false, "anchor": "| `v0.4.0` | Search hardening"} -->
```markdown
| `v0.5.0` | Query-shape fixes; no schema, engine or JS/NAPI change (schema stays 2, turso stays 0.7.2). The candidate fetch is a bare rowid lookup and scope, kind, extension and hidden/ignored/sensitive policy are applied in Rust **before** ranking and the result window (the 0.4.0 semantics, unchanged); stage-1 range scans use the covering primary-key index; non-ASCII tokens use a range instead of `LIKE` (a miss no longer scans the terms table); the rarest token drives multi-token queries and the others are verified against the path, so mixed-selectivity queries are exact instead of silently truncated. Additive API: `KuntuIndex::search_with_options` / `SearchOptions::fetch_budget` / `DEFAULT_FETCH_BUDGET` (5,000 admitted candidates, hard cap `MAX_KFS_CANDIDATE_ROWS` 50,000). `IndexedSearchOutcome::candidate_count` is now at most the budget and `coverage` is `Truncated` when the budget stops the search. Measured on a generated 1,000,000-row realistic fixture (release, Apple M4 Pro, external disk, best of 3): ASCII and non-ASCII misses 0.0 ms (non-ASCII was 15.5 to 21.5 s), 9-candidate query 0.5 ms (was 168 to 820 ms), 1,477-candidate query 7.8 ms (was 2.5 to 2.7 s for 205 candidates), 5,000-candidate broad query 40 ms (was 299 s for 20,270 candidates), exact 271-row mixed-selectivity query 39 ms (was truncated), 1-character prefix 44 ms. The 0.4.0 note about a "seek-capable range plan" is superseded: the range was always a seek, the candidate fetch was the cost. |
```

Run the GREEN checks of Task 4 again (nothing behavioural changed), then:

```
cargo fmt --all -- --check
cargo test --release -p kuntu-index --locked
```

**Commit:**

```
git add justfile README.md crates/kuntu-index/src/lib.rs
git commit -m "docs(kuntu-index): benchmark recipes, correct the stale 1M note, v0.5.0 revision row"
```


## Task 6: final verification (no new files)

Run, in this order, and paste the real output lines into the hand-back report:

```
cargo fmt --all -- --check
cargo clippy -p kuntu-index --all-targets --locked -- -D warnings
cargo test -p kuntu-index -p kuntu-core -p kuntu-daemon -p kuntu-cli -p kuntu-watcher --locked
cargo test --release -p kuntu-index --locked -- --ignored --nocapture --test-threads=1
KUNTU_BENCH_ROWS=1000000 KUNTU_BENCH_DIR=/Volumes/Portable2TB/tmp/kuntu-bench \
  cargo test --release -p kuntu-index --locked realistic_fixture_latency_gate -- --ignored --nocapture
git diff main --stat
git diff main --stat -- Cargo.toml Cargo.lock crates/kuntu-core crates/kuntu-napi crates/kuntu-daemon crates/kuntu-cli .github
```

Expected: fmt and clippy clean; the debug test run passes (this is the default CI command for these crates; the NAPI/Node leg of CI is untouched because no NAPI file changed); the ignored suite passes; the 1M gate passes with every class under its ceiling and prints the `GATE` lines to copy into the report; the first `git diff --stat` lists only the write set; the second is **empty**.

Not run by this plan (state it in the report): the full `cargo test --workspace` including `kuntu-scan` and `kuntu-napi`, the NAPI/Node smoke tests, Windows and Linux CI legs, and any Xross or Kunkun consumer test. Moving a consumer pin to the resulting SHA is a separate owner step (CONTRIBUTING.md).

## Open owner decisions for M1

The owner already accepted: ship M1 first on turso as v0.5.0, default fetch budget 5,000, SQLite for the later shared store. Still open, with recommended defaults (none blocks starting):

| # | Decision | Recommended default |
|---|---|---|
| 1 | The ranker is not optimised in M1. Scoring costs about 1.5 to 1.8 microseconds per candidate (about 9 ms at the 5,000 budget, 74 to 91 ms only when a caller asks for the 50,000 cap). | **Defer** to a later task; revisit only if a caller raises the budget. |
| 2 | Add a CI step that runs the ignored suite at 100k rows on the macOS leg (`cargo test --release -p kuntu-index --locked -- --ignored`, about 5 minutes after cache) so the recall and gate tests guard every PR. It changes `.github/workflows/ci.yml`, outside this write set. | **Yes**, as a small follow-up after the owner reviews the gate times. |
| 3 | `candidate_count` keeps its name and now means "candidates ranked", at most the budget. Xross reads `coverage`; no consumer parses the old 50,000 value. | **Keep**; mention it in the Xross pin-bump commit. |
| 4 | Redundant indexes (`idx_terms_term`, `idx_terms_entry`, `idx_entries_path`, `idx_entries_root_deleted_path_lower`, about 650 MB at 1M entries) stay until M2's schema v3. | **Leave** (a schema change belongs with the v3 file). |
| 5 | Keep the pathological single-term 60k/1M tests alongside the realistic gate. | **Keep** (they guard the 50,000 cap and the work bound). |
| 6 | Cut `v0.5.0` and move Xross's gitlink (`a5df355` to the new SHA) only after the owner runs `just benchmark-1m` on the internal disk. | **Yes**; the research disk was too slow to give internal-disk numbers. |


## Replay results (what a worker should see)

Environment: Apple M4 Pro, 14 cores, 48 GB, rustc/cargo 1.98.1, release builds, fixtures on an external disk, other agents running (load average above 10), so cold numbers are pessimistic. Clean clone of `main` at `4c5dbfd`; the edit blocks of this file were applied with a script in the order of the tasks, then `cargo fmt --all` before each commit. Branch `feat/m1-query-shape` of the replay clone ended with five commits (the five commit messages above).

| Step | Observed |
|---|---|
| Release build, cold dependencies | 4 min 18 s (first build in a fresh target directory about 12 minutes); later incremental builds 1 to 1.5 min |
| T1 RED, gate at 100k | fixture build 49 s (214 MB database plus a 2 GB turso write-ahead log); `miss` 0.0 ms passes; `miss_non_ascii` **759 ms against 25 ms: FAIL** |
| T1 GREEN | 26 passed, 3 ignored; clippy clean after one fix (`is_multiple_of`) |
| T2 RED | `non_ascii_prefixes_use_a_range_not_like` fails: `é: t.term LIKE ?`; the behaviour test passes |
| T2 GREEN | 28 passed; gate at 100k: `miss_non_ascii` 0.0 ms, `rare` 17.7 ms, `mid` 64 ms pass, **`broad` fails: 4.45 s against 400 ms** |
| T3 RED | compile errors: `DEFAULT_FETCH_BUDGET`, `SearchOptions`, `entry_fetch_sql`, `search_with_options` not found |
| T3 GREEN | 33 passed, 4 ignored; `default_budget_bounds_a_broad_single_token` passes (12 s including the 60k insert); the 60k single-term benchmark passes in 1.7 s (31 s before); gate at 100k passes all nine classes (`broad` 50 ms, `mixed` 74 ms, `all_rows` 177 ms, `prefix1` 134 ms); **gate at 300k fails at `mixed`: `left: 145, right: 169`, `Truncated`** (fixture build 272 s, 694 MB database plus a 2.5 GB log); at 1M rows `broad` takes 11.2 s cold and the gate fails there |
| T4 RED | `file_name_hits_are_fetched_before_path_only_hits...` fails (a result named `aaa-...`); `rarest_token_drives_the_search_and_recall_is_exact` fails `left: 8, right: 33`; the multi-token semantics test passes (guard) |
| T4 GREEN | 38 passed, 5 ignored; ignored recall, budget and 60k tests pass; clippy clean; gate at 100k and 300k pass every class (`mixed` exact: 169 of 169, `Complete`) |
| T5 | `cargo fmt --check` clean; suite 38 passed |
| T6 | debug run of `kuntu-index` (38 passed, 5 ignored), `kuntu-cli` (22), `kuntu-core` (21), `kuntu-daemon` (5), `kuntu-watcher` (3): all pass |

Gate at 1,000,000 rows with the final code (fixture build 882 s, 2.4 GB database plus a 2.9 GB write-ahead log; best of 3 runs, warm, milliseconds; the cold `run=0` is shown for reference):

| Class | Candidates | Cold run 0 | Best of 3 | Ceiling | Before (4c5dbfd, realistic 1M) |
|---|---|---|---|---|---|
| miss | 0 | 0.7 | 0.0 | 25 | 0.2 to 1.3 |
| miss, non-ASCII | 0 | 0.0 | 0.0 | 25 | 15,506 to 21,498 |
| rare | 9 | 5.3 | 0.5 | 50 | 168 to 820 (2 candidates) |
| mid | 1,477 | 246.5 | 7.8 | 100 | 2,483 to 2,676 (205 candidates) |
| broad | 5,000, `Truncated` | 669.5 | 39.8 | 400 | 298,718 (20,270 candidates) |
| prefix3 | 5,000, `Truncated` | 39.5 | 39.5 | 400 | not run (about an hour) |
| mixed (`mid ts`) | 271 = ground truth, `Complete` | 180.1 | 39.1 | 300 | wrong and `Truncated` above 200k rows |
| all_rows | 5,000, `Truncated` | 114.8 | 28.7 | 600 | not run (about 20 minutes) |
| prefix1 | 5,000, `Truncated` | 136.8 | 43.5 | 800 | not run |

The ceilings leave a factor of 9 to 17 over the warm numbers and still catch the old behaviour (seconds to minutes) by a wide margin. Notes for the owner run: the fixture cache needs about 6 GB free for 1M rows, because turso leaves a write-ahead log larger than the database after the bulk load; the first 1M run takes about 15 minutes, later runs about one minute.

A second, independent replay from the committed text of this file (fresh clone of `main`, edits applied by script, every RED and GREEN command above re-run, 100k fixture rebuilt from scratch in 32 s) reproduced every step: T1 RED `miss_non_ascii` 773 ms; T2 RED `é: t.term LIKE ?`, gate stops at `broad` 4.2 s; T3 RED 16 compile errors, GREEN 33 passed, gate at 300k fails `mixed` (145 vs 169); T4 RED `left: 8, right: 33`, GREEN 38 passed with the gate green at 100k and 300k; fmt and clippy clean; debug run of `kuntu-index` (38 passed, 5 ignored), `kuntu-cli` (22), `kuntu-core` (21), `kuntu-daemon` (5), `kuntu-watcher` (3). The full ignored suite (`just benchmark` equivalent: both single-term benchmarks, the budget and recall fixtures and the 100k gate) passes in 69 s; the 1M single-term benchmark that used to fail after 1,005.7 s now passes. The 1M gate again passed every class (warm best of 3, ms): miss 0.0, non-ASCII 0.0, rare 0.4, mid 7.8, broad 40.0, prefix3 39.9, mixed 37.8 (271, exact), all_rows 29.4, prefix1 42.9, on a machine with a load average above 11; cold first runs were 0.2 to 1,244 ms. Timing varies by about 2x with machine load (the same 100k gate gave `broad` 40 ms in one run and 115 ms in another), which is why the ceilings leave an order of magnitude.

