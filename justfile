
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
