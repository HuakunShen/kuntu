//! Strict M2 acceptance; performance misses are reported as open RED items.
use super::*;
fn ceiling_ms(class: &str, budget: usize) -> f64 {
  if budget == 50_000 {
    return 150.0;
  }
  match class {
    "miss" | "miss_non_ascii" => 2.0,
    "rare" | "mid" => 10.0,
    "prefix1" => 40.0,
    _ => 25.0,
  }
}
#[test]
fn m2_ceilings_reject_the_m1_only_latency_range() {
  assert_eq!(ceiling_ms("miss", 5000), 2.0);
  assert_eq!(ceiling_ms("rare", 5000), 10.0);
  assert_eq!(ceiling_ms("mid", 5000), 10.0);
  assert!(
    26.0 > ceiling_ms("broad", 5000),
    "M1-only 26 ms was accepted"
  );
  assert_eq!(ceiling_ms("prefix1", 5000), 40.0);
  assert!(151.0 > ceiling_ms("broad", 50000));
}
#[test]
#[ignore = "strict 1M M2 latency acceptance; retain every miss"]
fn realistic_fixture_m2_latency_acceptance() {
  let rows = std::env::var("KUNTU_BENCH_ROWS")
    .unwrap()
    .parse::<usize>()
    .unwrap();
  assert_eq!(rows, 1_000_000);
  let f = fixture::open_or_build(rows);
  let q = &f.queries;
  let classes = vec![
    ("miss", "zzqqx".to_string()),
    ("miss_non_ascii", "zzqqé".to_string()),
    ("rare", q.rare.clone()),
    ("mid", q.mid.clone()),
    ("broad", q.broad.clone()),
    ("prefix3", q.prefix3.clone()),
    ("mixed", q.mixed.clone()),
    ("all_rows", q.all_rows.clone()),
    ("prefix1", q.prefix1.clone()),
  ];
  let mut measurements = Vec::new();
  let mut misses = Vec::new();
  for budget in [5000, 50000] {
    for (class, text) in &classes {
      let query = SearchQuery::new(text).with_limit(20);
      let options = SearchOptions {
        fetch_budget: budget,
      };
      let mut count = 0;
      let best = fixture::measure_warm_best(|| {
        let start = std::time::Instant::now();
        let outcome = f
          .index
          .search_with_options(&f.config, &query, options)
          .unwrap();
        count = outcome.candidate_count;
        start.elapsed()
      });
      let ms = best.as_secs_f64() * 1000.0;
      let ceiling = ceiling_ms(class, budget);
      let pass = ms <= ceiling;
      eprintln!("M2_GATE class={class} budget={budget} best_ms={ms:.3} ceiling_ms={ceiling:.1} candidates={count} pass={pass}");
      measurements.push(serde_json::json!({"class":class,"budget":budget,"best_ms":ms,"ceiling_ms":ceiling,"candidates":count,"pass":pass}));
      if !pass {
        misses.push(format!("{class}/{budget} {ms:.3}>{ceiling} ms"));
      }
    }
  }
  let output = PathBuf::from(
    std::env::var_os("KUNTU_ACCEPTANCE_DIR").expect("acceptance artifacts under scratch"),
  );
  std::fs::create_dir_all(&output).unwrap();
  std::fs::write(
    output.join("m2-latency.json"),
    serde_json::to_vec_pretty(&measurements).unwrap(),
  )
  .unwrap();
  assert!(
    misses.is_empty(),
    "M2_ACCEPTANCE_MISS: {}",
    misses.join("; ")
  );
}
