# M2 shared-store consumer contract

M2's in-repository consumers are the CLI, daemon and NAPI hosts. The macOS smoke runs the CLI and daemon with two independent Node processes standing in for Xross (read-only) and Kunkun (requesting the writer lease). Actual product integration is M2-X and M2-K, in their repositories after M2 lands (D16).

Consumers resolve one absolute `KUNTU_HOME` (default `$HOME/.kuntu`), use `Home`/shared factories and keep the opened directory and epoch lifetime capability alive. They never open `index.v3.db` themselves or write SQL. Refusals are surfaced; there is no silent temporary-store fallback. Modes express access; `writerLeaseHeld()` is informational and is never authorization for a write.

A stable consumer name identifies persisted root membership. Registration is a small guarded write. Index traversal uses the union of enabled memberships; query admission uses only the requesting consumer's hidden/ignored/priority settings (false/false/0 by default). Every search supplies non-empty configured roots. The engine intersects them with persisted enabled memberships and any explicit Rust `SearchScope`. Callers cannot widen scope or allowances with a request. Remote-peer export mappings remain product-owned. Current compiled sensitive denial always applies, including while a policy reconciliation is interrupted.

Rust uses `KuntuIndex::open_shared`, `open_shared_minor` may only lower the compiled minor, and `open_shared(..., true, ...)` opens read-only. Every public read checks the current reader floor. `acquire_writer` rechecks floors after acquiring the capability and releases it on failure or downgrade. Every writer transaction checks the floor again inside `BEGIN`. Compatible older readers select explicit columns and continue after additive migration; incompatible versions surface `SchemaTooNew` or `ReadOnly`.

NAPI uses `FileSearchIndex.openShared(home, consumer)` for an embedded writer poller, `openClient(home, consumer)` for registration/inbox without a poller, and `openReadOnly(home, consumer)` for reads only. Read-only instances reject registration, requestRebuild, rebuild, refresh and repair on every task. `requestRebuild` queues work; shared rebuild/refresh/repair do not promise synchronous scan completion. Poll `status()` for `building`/`ready` and `search()` coverage; keep a pooled reader per instance. `close()` stops and joins the worker before its lease can leave the process. Outstanding tasks retain their capabilities until completion. The string constructor still opens exactly the supplied private v2/v3 path and preserves its private 50k compatibility budget.

CLI uses shared mode without `--db` and supports `store info|gc|doctor|import`, scoped search, index commands and daemon. `--db` preserves the private path behavior. The daemon accepts only its configured roots, serves concurrent read-only requests, queues shared bulk requests and runs the same persistent writer poller. Its HTTP prototype remains loopback by default; product helper transport/signing is a follow-up.

The poller tries every five seconds, owns every bulk path, and recovers persisted dirty/building roots only after acquisition. Restarted walks use fresh visitation generations; each batch and ready transition rechecks generation and policy. Cleanup seeks by rowid, inspects at most 5,000 rows and checks cancellation/deadline between rows. A stop leaves the root building, cancels the walk/cleanup, joins the worker, then releases the lease. An individual OS call, SQLite statement, commit or scheduling delay can exceed 100 ms; acceptance tests measure shutdown and reader latency rather than claim a universal real-time bound.

Required executable contract tests:

- `macos_four_host_shared_store_consumer_contract`: four live processes on one identity; CLI/daemon/NAPI searches; strict scope; read-only task rejection; queued rebuild; daemon-to-NAPI takeover within six seconds.
- `strict_consumer_never_inherits_union_or_client_flags` and `requested_scope_cannot_widen_config_even_on_an_early_return`: scope and allowance denial, including empty/miss paths.
- `preopened_old_writer_rechecks_after_migration_and_releases_capability`, `long_lived_rust_readers_recheck_floor_for_search_status_and_probes`: long-lived skew refusal and capability release.
- `shutdown_during_cleanup_is_bounded_and_readers_continue_before_takeover` and `restarted_walk_removes_deleted_and_renamed_committed_files`: interrupted completion and bounded graceful hand-off.
- Node `openReadOnly enforces access on rebuild refresh repair and registration` plus `old_constructor_opens_exact_v2_path_and_refuses_unrelated_database`: effective access and private compatibility.

Product lanes must reproduce these boundary conditions through their own hosts. This document supplies the shared-store contract and host smoke, not product integration evidence.
