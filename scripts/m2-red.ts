/** Exact defect replacements. RED must be a named test failure, then GREEN. */
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {strict as assert} from 'node:assert';
type Case={task:string,file:string,old:string,bad:string,crate:string,test:string};
const cases:Case[]=[
 {"task": "M2.10-reader-ceilings", "file": "scripts/m2-evidence.ts", "old": "if(m.max_us>100000)", "bad": "if(m.max_us>200000)", "crate": "reader-ceilings", "test": "m2_reader_ceilings_reject_a_100ms_overrun"},
 {"task": "M2.6-restart", "file": "crates/kuntu-index/src/shared.rs", "old": "active_scan_gen=active_scan_gen+1", "bad": "active_scan_gen=CASE WHEN status='building' AND scan_policy_revision=policy_revision THEN active_scan_gen ELSE active_scan_gen+1 END", "crate": "kuntu-index", "test": "m2_tests::restarted_walk_removes_deleted_and_renamed_committed_files"},
 {"task": "M2.4-preopened", "file": "crates/kuntu-index/src/shared.rs", "old": "      if floor > self.conn.minor {\n        self.conn.readonly.set(true);\n        return Err(IndexError::ReadOnly);\n      }\n", "bad": "      if false && floor > self.conn.minor {\n        self.conn.readonly.set(true);\n        return Err(IndexError::ReadOnly);\n      }\n", "crate": "kuntu-index", "test": "m2_tests::preopened_old_writer_rechecks_after_migration_and_releases_capability"},
 {"task": "M2.7-downgrade", "file": "crates/kuntu-index/src/shared.rs", "old": "    if self.legacy.is_none() {\n      let floor:", "bad": "    if false && self.legacy.is_none() {\n      let floor:", "crate": "kuntu-index", "test": "m2_tests::holder_service_drops_unusable_capability_after_writer_floor_changes"},
 {"task": "M2.9-self-import", "file": "crates/kuntu-index/src/shared.rs", "old": "    let leaf = format!(\n      \"import-{}.db\",\n      random\n        .iter()\n        .map(|v| format!(\"{v:02x}\"))\n        .collect::<String>()\n    );\n", "bad": "    let leaf = String::from(\"import-v2.db\");\n", "crate": "kuntu-index", "test": "m2_tests::self_snapshot_import_uses_unique_inodes_and_preserves_all_source_hashes"},
 {"task": "M2.4-durable-marker", "file": "crates/kuntu-store/src/lib.rs", "old": "      self.dir.sync_all()?;\n      namespace_event(\"marker_directory_synced\");", "bad": "      namespace_event(\"marker_sync_skipped\");", "crate": "kuntu-store", "test": "tests::durable_markers_precede_every_namespace_member_and_only_enoent_is_absent"},
 {"task": "M2.4-gc-reuse", "file": "crates/kuntu-store/src/lib.rs", "old": "    if epoch < EPOCH && self.exists(&format!(\"gc-{epoch}.json\"))? {", "bad": "    if false && epoch < EPOCH && self.exists(&format!(\"gc-{epoch}.json\"))? {", "crate": "kuntu-store", "test": "tests::pending_gc_is_completed_before_an_old_epoch_can_be_reused"},
 {"task": "M2.3-sensitive-admission", "file": "crates/kuntu-index/src/lib.rs", "old": "      || kuntu_core::denies_sensitive_path(&entry.path)", "bad": "", "crate": "kuntu-index", "test": "m2_tests::current_sensitive_policy_denies_historical_rows_during_interrupted_reconciliation"},
 {"task": "M2.3-policy-invalidation", "file": "crates/kuntu-index/src/shared.rs", "old": "    if stored == identity {", "bad": "    if true || stored == identity {", "crate": "kuntu-index", "test": "m2_tests::current_sensitive_policy_denies_historical_rows_during_interrupted_reconciliation"},
 {"task": "M2.4-rust-reader", "file": "crates/kuntu-index/src/shared.rs", "old": "    if min > self.conn.minor {", "bad": "    if false && min > self.conn.minor {", "crate": "kuntu-index", "test": "m2_tests::long_lived_rust_readers_recheck_floor_for_search_status_and_probes"},
 {"task": "M2.5-cleanup-seek", "file": "crates/kuntu-index/src/shared.rs", "old": "SELECT id,root_id,scan_gen FROM entries WHERE id>?1 ORDER BY id LIMIT 5000", "bad": "SELECT id,root_id,scan_gen FROM entries WHERE scan_gen=0 AND id>?1 ORDER BY id LIMIT 5000", "crate": "kuntu-index", "test": "m2_tests::cleanup_seek_limits_inspection_even_across_large_current_generation_gaps"},
 {"task": "M2.7-cleanup-cancel", "file": "crates/kuntu-index/src/shared.rs", "old": "    if self.cancel.load(Ordering::Acquire) {", "bad": "    if false && self.cancel.load(Ordering::Acquire) {", "crate": "kuntu-index", "test": "m2_tests::shutdown_during_cleanup_is_bounded_and_readers_continue_before_takeover"},
 {"task": "M2.10-four-host", "file": "crates/kuntu-napi/src/lib.rs", "old": "      _service: (!readonly).then(|| Arc::new(WriterService::start(h))),", "bad": "      _service: None,", "crate": "smoke", "test": "macos_four_host_shared_store_consumer_contract"},
 {"task": "M2.10-ceilings", "file": "crates/kuntu-index/src/m2_acceptance.rs", "old": "    _ => 25.0,", "bad": "    _ => 400.0,", "crate": "kuntu-index", "test": "m2_acceptance::m2_ceilings_reject_the_m1_only_latency_range"},
 {"task": "M2.10-internal-ceiling", "file": "scripts/m2-internal-rebuild.ts", "old": "const secondsCeiling=120;", "bad": "const secondsCeiling=999;", "crate": "internal", "test": "internal_ssd_one_million_rebuild_within_120_seconds_ceiling_contract"},

 {task:'M2.4-client-version',file:'crates/kuntu-index/src/shared.rs',old:'if !(0..=BUILD_MINOR).contains(&minor) {',bad:'if minor < 0 {',crate:'kuntu-index',test:'m2_tests::caller_cannot_claim_a_future_build_minor_to_bypass_writer_floor'},
 {"task": "M2.2-exec", "file": "crates/kuntu-store/src/lib.rs", "old": "    let mut fd = create_lock_file(&locks, || {\n      openat(\n        &locks,\n        name,\n        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,\n        Mode::from_bits_truncate(0o600),\n      )\n    })?;\n", "bad": "    let mut fd = create_lock_file(&locks, || {\n      openat(\n        &locks,\n        name,\n        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NOFOLLOW,\n        Mode::from_bits_truncate(0o600),\n      )\n    })?;\n", "crate": "kuntu-store", "test": "tests::executed_child_does_not_inherit_the_permanent_writer_lease"},
 {task:'M2.1-lock-create',file:'crates/kuntu-store/src/lib.rs',old:'attempts < 100',bad:'attempts < 0',crate:'kuntu-store',test:'tests::transient_lock_creation_retries_only_not_found_under_the_same_directory_fd'},
 {task:'M2.5-cleanup',file:'crates/kuntu-index/src/shared.rs',old:'inspected >= 5000 || started.elapsed() >= std::time::Duration::from_millis(100)',bad:'inspected >= 5000',crate:'kuntu-index',test:'m2_tests::slow_delete_triggers_force_the_100_ms_cleanup_boundary'},
 {task:'M2.1',file:'crates/kuntu-store/src/lib.rs',old:'let flags = OFlag::O_RDWR\n      | OFlag::O_NOFOLLOW',bad:'let flags = OFlag::O_RDWR',crate:'kuntu-store',test:'tests::descriptor_open_rejects_symlink_and_private_modes_are_exact'},
 {task:'M2.2',file:'crates/kuntu-store/src/lib.rs',old:'else if !fd.try_lock_exclusive()? {',bad:'else if false && !fd.try_lock_exclusive()? {',crate:'kuntu-store',test:'tests::permanent_lease_inode_is_exclusive_and_reusable'},
 {task:'M2.3',file:'crates/kuntu-index/src/lib.rs',old:'SELECT entry_id FROM {} WHERE term >= ?1 AND term < ?2 LIMIT ?3',bad:'SELECT entry_id FROM {} WHERE term >= ?1 AND term < ?2 LIMIT ?1',crate:'kuntu-index',test:'m2_tests::production_builders_pin_both_postings_reverse_and_rowid'},
 {task:'M2.4',file:'crates/kuntu-index/src/shared.rs',old:'if read_min > minor {',bad:'if false && read_min > minor {',crate:'kuntu-index',test:'m2_tests::actual_opens_enforce_skew_before_wal_and_old_writer_steps_down'},
 {task:'M2.5',file:'crates/kuntu-index/src/engine.rs',old:"UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='generation'",bad:"UPDATE meta SET value=CAST(value AS INTEGER)+0 WHERE key='generation'",crate:'kuntu-index',test:'m2_tests::batch_generation_is_atomic_and_readers_observe_data_version'},
 {task:'M2.6',file:'crates/kuntu-index/src/shared.rs',old:"status='building' AND active_scan_gen=?2 AND scan_policy_revision=policy_revision",bad:"status='building' AND ?2=?2 AND scan_policy_revision=policy_revision",crate:'kuntu-index',test:'m2_tests::restart_uses_fresh_generation_stale_completion_is_rejected_and_building_is_reported'},
 {task:'M2.7',file:'crates/kuntu-index/src/shared.rs',old:'let _ = t.join();',bad:'drop(t);',crate:'kuntu-index',test:'m2_tests::joined_takeover_service_releases_lease_only_after_worker_stops'},
 {task:'M2.8',file:'crates/kuntu-napi/src/lib.rs',old:'read_only: true,',bad:'read_only: false,',crate:'node',test:'openReadOnly enforces access on rebuild refresh repair and registration'},
 {task:'M2.9',file:'crates/kuntu-daemon/src/lib.rs',old:'("POST", "/index/rebuild") => match parse_roots_request(&request.body, allowed_roots) {\n      Ok(config) => {\n        if index.is_shared() {',bad:'("POST", "/index/rebuild") => match parse_roots_request(&request.body, allowed_roots) {\n      Ok(config) => {\n        if false && index.is_shared() {',crate:'kuntu-daemon',test:'tests::non_holder_bulk_endpoint_queues_and_empty_search_is_rejected'},
 {task:'M2.10',file:'crates/kuntu-cli/src/args.rs',old:'"store" => {',bad:'"store-disabled" => {',crate:'kuntu-cli',test:'args::tests::store_import_parser_accepts_source_and_rejects_missing_source'},
];
const base='/Volumes/Portable2TB/tmp/kuntu-m2/rev3/replay';mkdirSync(base,{recursive:true});
function command(args:string[],cwd?:string){const r=spawnSync(args[0]!,args.slice(1),{cwd,env:process.env,encoding:'utf8',maxBuffer:16*1024*1024});return {status:r.status,text:r.stdout+r.stderr};}
function test(c:Case){
 if(c.crate==='reader-ceilings')return command(['bun','scripts/m2-evidence.ts','ceilings']);
 if(c.crate==='internal')return command(['bun','scripts/m2-internal-rebuild.ts','--contracts']);
 if(c.crate==='smoke'){const b=command(['pnpm','run','build:debug'],'crates/kuntu-napi');assert.equal(b.status,0,b.text);return command(['bun','scripts/m2-consumer-smoke.ts']);}
 if(c.crate==='node'){
  const b=command(['pnpm','run','build:debug'],'crates/kuntu-napi');assert.equal(b.status,0,b.text);
  return command(['node','--test','--test-name-pattern',c.test,'test/index.test.cjs'],'crates/kuntu-napi');
 }
 return command(['cargo','test','-p',c.crate,c.test,'--','--exact','--test-threads=1','--nocapture']);
}
const chosen=process.argv[2];
for(const c of cases.filter(c=>!chosen||c.task===chosen)){
 const original=readFileSync(c.file,'utf8');
 const n=original.split(c.old).length-1;assert.equal(n,1,`anchor ${c.task} count=${n}`);
 try {
  writeFileSync(c.file,original.replace(c.old,c.bad));const red=test(c);writeFileSync(`${base}/${c.task}-RED.log`,red.text);
  assert.notEqual(red.status,0,`${c.task} RED unexpectedly passed`);assert.ok(red.text.includes(c.test),red.text);assert.match(red.text,/FAILED|not ok|✖|AssertionError/);assert.doesNotMatch(red.text,/could not compile|error\[E\d+\]/);
 } finally {writeFileSync(c.file,original);}
 const green=test(c);writeFileSync(`${base}/${c.task}-GREEN.log`,green.text);assert.equal(green.status,0,green.text);
 if(!['node','smoke','internal','reader-ceilings'].includes(c.crate))assert.match(green.text,/1 passed; 0 failed/);
 else if(c.crate!=='node')assert.ok(green.text.includes('PASS '+c.test),green.text);
 console.log(`${c.task}: RED named failure, GREEN passed (${c.test})`);
}
