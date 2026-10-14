use super::*;
#[test]
fn search_retains_only_top_k_results() {
  let mut index = KuntuIndex::open_memory().unwrap();
  let root = SearchRoot::new("/Users/kuntu/topk");
  let id = index.register_root(&root).unwrap();
  assert!(index.acquire_writer().unwrap());
  let specs: Vec<_> = (0..64)
    .map(|n| fixture::RowSpec {
      path: format!("/Users/kuntu/topk/target-{n}.md"),
      is_dir: false,
    })
    .collect();
  fixture::insert_rows(&mut index, id, &specs, 1);
  let config = SearchConfig { roots: vec![root] };
  let outcome = index
    .search_with_metrics(&config, &SearchQuery::new("target").with_limit(3))
    .unwrap();
  assert_eq!(outcome.candidate_count, 64);
  assert_eq!(outcome.results.len(), 3);
  assert!(outcome.truncated);
  assert!(
    index.last_search_profile().peak_retained_results <= 3,
    "ranking retained {:?}",
    index.last_search_profile()
  );
}
#[test]
fn candidate_fetch_does_not_copy_owned_sql_rows() {
  let mut index = KuntuIndex::open_memory().unwrap();
  let root = SearchRoot::new("/Users/kuntu/rowcopies");
  let id = index.register_root(&root).unwrap();
  assert!(index.acquire_writer().unwrap());
  let specs: Vec<_> = (0..64)
    .map(|n| fixture::RowSpec {
      path: format!("/Users/kuntu/rowcopies/target-{n}.md"),
      is_dir: false,
    })
    .collect();
  fixture::insert_rows(&mut index, id, &specs, 1);
  engine::OWNED_ROW_COPIES.with(|n| n.set(0));
  let outcome = index
    .search_with_metrics(
      &SearchConfig { roots: vec![root] },
      &SearchQuery::new("target"),
    )
    .unwrap();
  assert_eq!(outcome.candidate_count, 64);
  engine::OWNED_ROW_COPIES
    .with(|n| assert!(n.get() < 10, "copied {} SQL rows before admission", n.get()));
}
fn classes(q: &fixture::FixtureQueries) -> Vec<(&'static str, String)> {
  vec![
    ("miss", "zzqqx".into()),
    ("miss_non_ascii", "zzqqé".into()),
    ("rare", q.rare.clone()),
    ("mid", q.mid.clone()),
    ("broad", q.broad.clone()),
    ("prefix3", q.prefix3.clone()),
    ("mixed", q.mixed.clone()),
    ("all_rows", q.all_rows.clone()),
    ("prefix1", q.prefix1.clone()),
  ]
}
#[test]
#[ignore = "1M phase profile, release only"]
fn realistic_fixture_search_phase_profile() {
  let f = fixture::open_or_build(1_000_000);
  for budget in [5000, 50000] {
    for (class, text) in classes(&f.queries) {
      let query = SearchQuery::new(text).with_limit(20);
      f.index
        .search_with_options(
          &f.config,
          &query,
          SearchOptions {
            fetch_budget: budget,
          },
        )
        .unwrap();
      f.index
        .search_with_options(
          &f.config,
          &query,
          SearchOptions {
            fetch_budget: budget,
          },
        )
        .unwrap();
      let p = f.index.last_search_profile();
      eprintln!("M2_PHASE class={class} budget={budget} postings_ms={:.3} fetch_admission_ms={:.3} scoring_ms={:.3} sorting_ms={:.3} peak_results={}",p.postings_ns as f64/1e6,p.fetch_admission_ns as f64/1e6,p.scoring_ns as f64/1e6,p.sorting_ns as f64/1e6,p.peak_retained_results);
    }
  }
}
#[test]
#[ignore = "1M exact before/after result and coverage comparison, release only"]
fn realistic_fixture_search_results_equal_before() {
  let f = fixture::open_or_build(1_000_000);
  let mut cases = Vec::new();
  for budget in [5000, 50000] {
    for limit in [0, 1, 20, 50000] {
      for (class, text) in classes(&f.queries) {
        let outcome = f
          .index
          .search_with_options(
            &f.config,
            &SearchQuery::new(text).with_limit(limit),
            SearchOptions {
              fetch_budget: budget,
            },
          )
          .unwrap();
        let results: Vec<_> = outcome
          .results
          .iter()
          .map(|r| {
            serde_json::json!([
              r.path.to_string_lossy(),
              r.score,
              r.provider,
              format!("{:?}", r.matches),
              format!("{:?}", r.kind),
              r.byte_size,
              r.modified_unix_seconds
            ])
          })
          .collect();
        cases.push(serde_json::json!({"class":class,"budget":budget,"limit":limit,"candidate_count":outcome.candidate_count,"truncated":outcome.truncated,"coverage":format!("{:?}",outcome.coverage),"results":results}));
      }
    }
  }
  let bytes = serde_json::to_vec(&cases).unwrap();
  let path = PathBuf::from(
    std::env::var_os("KUNTU_SEARCH_BASELINE").expect("baseline artifact under scratch"),
  );
  if std::env::var("KUNTU_CAPTURE_SEARCH_BASELINE")
    .ok()
    .as_deref()
    == Some("1")
  {
    std::fs::write(&path, zstd::encode_all(bytes.as_slice(), 3).unwrap()).unwrap();
    eprintln!(
      "CAPTURED 72 unchanged-search result/coverage cases at {}",
      path.display()
    );
  } else {
    let expected = zstd::decode_all(std::fs::read(path).unwrap().as_slice()).unwrap();
    // Compare case-by-case without dumping hundreds of thousands of rows.
    let expected: Vec<serde_json::Value> = serde_json::from_slice(&expected).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (before, after) in expected.iter().zip(&cases) {
      assert!(
        before == after,
        "result/coverage changed for class={} budget={} limit={}",
        after["class"],
        after["budget"],
        after["limit"]
      );
    }
    eprintln!("EQUAL: all 72 exact result/coverage cases, including full admitted result sets");
  }
}
