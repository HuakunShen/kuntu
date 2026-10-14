/** Applies only the committed complete-file payload, then replays every task. */
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {dirname,resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
import {strict as assert} from 'node:assert';
import {createHash} from 'node:crypto';
const planPath=process.argv[2];assert.ok(planPath,'pass the git-show committed plan file');
const plan=readFileSync(planPath,'utf8');
const base='/Volumes/Portable2TB/tmp/kuntu-m2/rev3/replay';mkdirSync(base,{recursive:true});
process.env.CARGO_TARGET_DIR='/Volumes/Portable2TB/tmp/kuntu-m2/target';
process.env.KUNTU_TEST_DIR='/Volumes/Portable2TB/tmp/kuntu-m2/rev3/contracts';
process.env.KUNTU_ACCEPTANCE_DIR='/Volumes/Portable2TB/tmp/kuntu-m2/rev3/acceptance';
process.env.TMPDIR='/Volumes/Portable2TB/tmp/kuntu-m2/testtmp';mkdirSync(process.env.TMPDIR,{recursive:true});
process.env.KUNTU_BENCH_DIR='/Volumes/Portable2TB/tmp/kuntu-m2/bench-final';process.env.KUNTU_BENCH_ROWS='1000000';
const pattern=/<!-- apply ({[^\n]+}) -->\n(`{3,})[^\n]*\n([\s\S]*?)\n\2/g;
const files=new Map<string,string>();
for(const match of plan.matchAll(pattern)){
 const meta:unknown=JSON.parse(match[1]!);
 assert.ok(typeof meta==='object'&&meta!==null&&'file'in meta&&'mode'in meta&&'task'in meta);
 assert.ok(typeof meta.file==='string'&&typeof meta.task==='string'&&meta.mode==='file');
 assert.ok(!meta.file.startsWith('/')&&!meta.file.split('/').includes('..'));
 assert.ok(!files.has(meta.file),`duplicate complete payload ${meta.file}`);
 files.set(meta.file,match[3]!+'\n');
}
const setSection=plan.split('## Exact write set\n')[1]?.split('\n## ')[0];assert.ok(setSection);
const declared=[...setSection.matchAll(/^- `([^`]+)`/gm)].map(m=>m[1]!);
assert.deepEqual([...files.keys()].sort(),declared.sort(),'plan_apply_write_set_matches_complete_payloads');
for(const [file,contents]of files){mkdirSync(dirname(file),{recursive:true});writeFileSync(file,contents);}
const hashes=Object.fromEntries([...files].map(([f,s])=>[f,createHash('sha256').update(s).digest('hex')]));
writeFileSync(`${base}/payload-sha256.json`,JSON.stringify(hashes,null,2));
function run(label:string,args:string[],cwd?:string,extra:Record<string,string>={}){
 console.log(`RUN ${label}`);const r=spawnSync(args[0]!,args.slice(1),{cwd,env:{...process.env,...extra},encoding:'utf8',maxBuffer:64*1024*1024});const text=r.stdout+r.stderr;writeFileSync(`${base}/${label}.log`,text);assert.equal(r.status,0,`${label}\n${text.slice(-12000)}`);return text;
}
run('fmt',['cargo','fmt','--all','--','--check']);
run('clippy',['cargo','clippy','--workspace','--all-targets','--locked','--','-D','warnings']);
run('napi-install',['pnpm','install','--frozen-lockfile'],'crates/kuntu-napi');
run('cli-build',['cargo','build','--release','-p','kuntu-cli']);
run('reds-greens',['bun','scripts/m2-red.ts']);
run('workspace',['cargo','test','--workspace','--locked','--','--test-threads=1']);
const js=readFileSync('crates/kuntu-napi/index.js');const dts=readFileSync('crates/kuntu-napi/index.d.ts');
run('napi-build',['pnpm','run','build:debug'],'crates/kuntu-napi');
assert.deepEqual(readFileSync('crates/kuntu-napi/index.js'),js,'generated JS differs');assert.deepEqual(readFileSync('crates/kuntu-napi/index.d.ts'),dts,'generated declarations differ');
run('node',['node','--test','test/index.test.cjs'],'crates/kuntu-napi');run('types',['pnpm','run','typecheck'],'crates/kuntu-napi');
run('m1-gate',['cargo','test','--release','-p','kuntu-index','realistic_fixture_latency_gate','--','--ignored','--nocapture']);
const m2=spawnSync('cargo',['test','--release','-p','kuntu-index','realistic_fixture_m2_latency_acceptance','--','--ignored','--nocapture'],{env:process.env,encoding:'utf8',maxBuffer:64*1024*1024});
writeFileSync(`${base}/m2-gate.log`,m2.stdout+m2.stderr);
if(m2.status!==0){assert.match(m2.stdout+m2.stderr,/M2_ACCEPTANCE_MISS/);assert.doesNotMatch(m2.stdout+m2.stderr,/could not compile|error\[E\d+\]/);console.log('OPEN RED: realistic_fixture_m2_latency_acceptance; see measured acceptance JSON');}
const internal=spawnSync('bun',['scripts/m2-internal-rebuild.ts'],{env:process.env,encoding:'utf8'});writeFileSync(`${base}/internal-rebuild.log`,internal.stdout+internal.stderr);
if(internal.status===2){assert.match(internal.stdout,/M2_INTERNAL_SSD_NOT_RUN/);console.log('PENDING: internal_ssd_one_million_rebuild_within_120_seconds');}else assert.equal(internal.status,0,internal.stdout+internal.stderr);
writeFileSync(`${base}/acceptance-status.json`,JSON.stringify({m2_latency:m2.status===0?'PASS':'OPEN RED',internal_ssd:internal.status===2?'NOT RUN':'PASS'},null,2));
run('m1-recall' ,['cargo','test','--release','-p','kuntu-index','rarest_token_drives_the_search_and_recall_is_exact','--','--ignored','--nocapture']);
run('m1-budget',['cargo','test','--release','-p','kuntu-index','default_budget_bounds_a_broad_single_token','--','--ignored','--nocapture']);
run('sqlite-version',['cargo','test','--release','-p','kuntu-index','bundled_runtime_includes_wal_reset_fix','--','--nocapture']);
run('size-delete',['cargo','test','--release','-p','kuntu-index','realistic_fixture_reverse_delete_cost_and_size_gate','--','--ignored','--nocapture']);
run('probe-build',['cargo','build','--release','-p','kuntu-index','--example','m2_probe']);run('crash',['bun','scripts/m2-evidence.ts','crash']);
const stress=spawnSync('bun',['scripts/m2-evidence.ts','stress'],{env:process.env,encoding:'utf8',maxBuffer:64*1024*1024});writeFileSync(`${base}/stress.log`,stress.stdout+stress.stderr);
if(stress.status!==0){assert.match(stress.stdout+stress.stderr,/M2_STRESS_ACCEPTANCE_MISS/);console.log('OPEN RED: M2 stress reader latency; unchanged 5 ms p99 / 100 ms maximum ceilings');}
writeFileSync(`${base}/acceptance-status.json`,JSON.stringify({m2_latency:m2.status===0?'PASS':'OPEN RED',internal_ssd:internal.status===2?'NOT RUN':'PASS',stress_reader_latency:stress.status===0?'PASS':'OPEN RED'},null,2));
run('skew',['bun','scripts/m2-evidence.ts','skew']);
run('consumer-smoke',['bun','scripts/m2-consumer-smoke.ts']);
run('previous-tag',['bun','scripts/m2-previous-tag.ts']);
run('fmt-final',['cargo','fmt','--all','--','--check']);
const protectedPaths=['crates/kuntu-crawler','crates/kuntu-watcher','crates/kuntu-scan','.github'];
assert.equal(run('protected',['git','diff','--name-only','e164d67','--',...protectedPaths]).trim(),'');assert.equal(run('protected-untracked',['git','ls-files','--others','--exclude-standard','--',...protectedPaths]).trim(),'');
for(const [f,hash]of Object.entries(hashes))assert.equal(createHash('sha256').update(readFileSync(f)).digest('hex'),hash,`payload changed ${f}`);
console.log(`PASS: plan_apply_write_set_matches_complete_payloads; ${files.size} files; all correctness tasks and evidence commands completed; M2 acceptance status recorded separately`);
