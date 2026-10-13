/** Process crash/stress evidence. All children are awaited and killed on failure. */
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { mkdirSync, writeFileSync, existsSync, readFileSync, rmSync, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join, resolve } from 'node:path';
import { strict as assert } from 'node:assert';
const scratch=process.env.KUNTU_BENCH_DIR??'/Volumes/Portable2TB/tmp/kuntu-m2impl-bench/bench';
const base=process.env.KUNTU_EVIDENCE_DIR??join(scratch,'evidence');
const fixtureBase=process.env.KUNTU_EVIDENCE_FIXTURE_DIR??join(scratch,'evidence-fixtures');
const target=process.env.CARGO_TARGET_DIR??'target';
const bin=resolve(target,'release/examples/m2_probe');
mkdirSync(base,{recursive:true});
const root=join(fixtureBase,'root-200k');mkdirSync(root,{recursive:true});
const live=new Set<ChildProcess>();const orphaned=new Set<number>();
function start(op:string,home:string,extra:Record<string,string>={},minor='1') {
 const child=spawn(bin,[op,home,extra.KUNTU_EVIDENCE_ROOT??root,minor],{env:{...process.env,...extra},stdio:['ignore','pipe','pipe']});live.add(child);
 let output='';child.stdout?.on('data',(b:Buffer)=>{output+=b.toString()});child.stderr?.on('data',(b:Buffer)=>{output+=b.toString()});
 const done=new Promise<{code:number|null,text:string}>(r=>child.on('close',(code)=>{live.delete(child);r({code,text:output})}));
 return {child,done};
}
async function ok(op:string,home:string,extra:Record<string,string>={},minor='1') {const r=await start(op,home,extra,minor).done;assert.equal(r.code,0,r.text);return r.text.trim();}
async function marker(path:string,proc:ReturnType<typeof start>) {
 const deadline=Date.now()+600_000;while(!existsSync(path)){assert.equal(proc.child.exitCode,null,'child exited before handshake');assert.ok(Date.now()<deadline,'handshake timeout');await Bun.sleep(10);}
}
async function killed(op:string,home:string,point:string,rows?:number) {
 const m=join(base,`marker-${point}-${rows??0}`);rmSync(m,{force:true});
 const p=start(op,home,{KUNTU_FAULT:point,KUNTU_FAULT_MARKER:m,...(rows===undefined?{}:{KUNTU_FAULT_ROWS:String(rows)})});
 await marker(m,p);p.child.kill('SIGKILL');const r=await p.done;assert.equal(r.code,null);return readFileSync(m,'utf8');
}
function db(home:string,sql:string){const r=spawnSync('python3',['-c','import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); print(c.execute(sys.argv[2]).fetchall()); c.commit()',join(home,'index.v3.db'),sql],{encoding:'utf8',maxBuffer:256*1024*1024});assert.equal(r.status,0,r.stderr);return r.stdout.trim();}
type ReaderMetric={queries:number,errors:number,p99_us:number,max_us:number,committed_entries_at_start?:number,sampling_start_us?:number,sampling_end_us?:number,sampling_duration_us?:number,cpu_total_us?:number,probe_wall_total_us?:number,sqlite_retries?:number,sqlite_sleep_total_us?:number,storage_syscall_total_us?:number,outliers?:Record<string,number|string>[]};
type WriterEvent={phase:string,start_us:number,end_us:number,entries?:number,details?:Record<string,number|string>};
function stressEvidence(metrics:ReaderMetric[],events:WriterEvent[]):string[]{
 const misses=readerAcceptanceMisses(metrics);
 for(const m of metrics){
  assert.ok(m.queries>0 && (m.committed_entries_at_start??0)>0,'sampling must begin on a populated committed store');
  assert.ok((m.sampling_duration_us??0)>=60_000_000,'sampling must last at least 60 seconds');
  assert.ok((m.cpu_total_us??0)>0 && (m.probe_wall_total_us??0)>0,'missing CPU/wall measurements');
  assert.ok(Number.isFinite(m.sampling_start_us)&&Number.isFinite(m.sampling_end_us),'missing sample timestamps');
  for(const phase of ['batch_commit','checkpoint','cleanup']){
   assert.ok(events.some(e=>e.phase===phase&&e.start_us<=m.sampling_end_us!&&e.end_us>=m.sampling_start_us!),`no ${phase} overlap with sampling`);
  }
 }
 return misses;
}
function readEvents(path:string):WriterEvent[]{
 if(!existsSync(path))return [];
 const text=readFileSync(path,'utf8');
 return text.slice(0,text.lastIndexOf('\n')+1).split('\n').filter(Boolean).map(line=>JSON.parse(line) as WriterEvent);
}
function machineLoad(){const r=spawnSync('uptime',[],{encoding:'utf8'});assert.equal(r.status,0,r.stderr);return r.stdout.trim();}

function readerAcceptanceMisses(metrics:ReaderMetric[]):string[] {
 const misses:string[]=[];
 for(const m of metrics){
  assert.equal(m.errors,0,'reader errors are a correctness failure');
  if(m.p99_us>5000)misses.push(`p99 ${m.p99_us}>5000 us`);
  if(m.max_us>100000)misses.push(`maximum ${m.max_us}>100000 us`);
 }
 return misses;
}
const mode=process.argv[2]??'crash';
try {
 if(mode==='population-contracts') {
  const name=process.argv[3];console.log(`RUN ${name}`);
  const populated:ReaderMetric={queries:1,errors:0,p99_us:1,max_us:1,committed_entries_at_start:5000,sampling_start_us:10,sampling_end_us:60_000_100,sampling_duration_us:60_000_090,cpu_total_us:1,probe_wall_total_us:2};
  if(name==='reader_sampling_requires_committed_entries') {
   assert.throws(()=>stressEvidence([{...populated,committed_entries_at_start:0}],[]),/populat/);
  }else if(name==='sampling_requires_overlapping_writer_phases') {
   assert.throws(()=>stressEvidence([populated],[]),/overlap/);
   const phases=['batch_commit','checkpoint','cleanup'];
   const events=phases.map(phase=>({phase,start_us:20,end_us:30}));
   assert.equal(stressEvidence([populated],events).length,0);
   for(const phase of phases)assert.throws(()=>stressEvidence([populated],events.filter(e=>e.phase!==phase)),/overlap/);
   assert.throws(()=>stressEvidence([populated],events.map(e=>({...e,start_us:70_000_000,end_us:70_000_010}))),/overlap/);
  }else if(name==='empty_reader_exits_before_sampling') {
   const home=join(base,'empty-reader-contract');rmSync(home,{recursive:true,force:true});await ok('init',home);
   const r=spawnSync(bin,['reader',home,root,'1'],{env:process.env,encoding:'utf8',timeout:1000});
   assert.ok(r.status!==null&&r.status!==0&&r.stderr.includes('committed entries'),'reader sampled an empty database instead of refusing before sampling');
  }else throw new Error(`Unknown contract ${name}`);
  console.log(`PASS ${name}`);
 } else if(mode==='ceilings') {
  const name='m2_reader_ceilings_reject_a_100ms_overrun';console.log(`RUN ${name}`);
  assert.equal(readerAcceptanceMisses([{queries:1,errors:0,p99_us:5000,max_us:100000}]).length,0);
  assert.equal(readerAcceptanceMisses([{queries:1,errors:0,p99_us:100,max_us:100001}]).length,1,'100 ms ceiling was loosened');
  assert.equal(readerAcceptanceMisses([{queries:1,errors:0,p99_us:5001,max_us:100}]).length,1);
  assert.throws(()=>readerAcceptanceMisses([{queries:1,errors:1,p99_us:1,max_us:1}]),/correctness/);
  console.log(`PASS ${name}`);
 } else if(mode==='crash') {
  for(let n=0;n<200_000;n++){const p=join(root,`file-${String(n).padStart(6,'0')}.md`);if(!existsSync(p))writeFileSync(p,'x');}
  const home=join(base,'crash-home');rmSync(home,{recursive:true,force:true});
  await ok('init',home);const outcomes:Record<string,string>={};
  for(const percent of [30,70]){
   rmSync(home,{recursive:true,force:true});await ok('init',home);await killed('scan',home,'batch_committed',200_000*percent/100);
   const partialCount=Number(/\d+/.exec(db(home,"SELECT count(*) FROM entries"))?.[0]);assert.ok(partialCount>=200_000*percent/100&&partialCount<200_000*percent/100+5000);
   const partial=db(home,"SELECT status,active_scan_gen,(SELECT value FROM meta WHERE key='generation'),(SELECT count(*) FROM entries) FROM root_state");
   const selected=spawnSync('python3',['-c','import sqlite3,json,sys; c=sqlite3.connect(sys.argv[1]); print(json.dumps([r[0] for r in c.execute("SELECT path FROM entries WHERE kind=1 ORDER BY id LIMIT 2")]))',join(home,'index.v3.db')],{encoding:'utf8'});assert.equal(selected.status,0,selected.stderr);
   const committed:unknown=JSON.parse(selected.stdout);assert.ok(Array.isArray(committed)&&committed.length===2&&committed.every(p=>typeof p==='string'));
   const deleted=String(committed[0]),renamed=String(committed[1]),replacement=renamed.replace('.md','-renamed.md');rmSync(deleted);const moved=readFileSync(renamed);rmSync(renamed);writeFileSync(replacement,moved);
   const resumed=await ok('scan',home);assert.equal(db(home,'SELECT count(*) FROM entries'),'[(200000,)]');assert.equal(db(home,"SELECT count(*) FROM entries WHERE scan_gen<>(SELECT active_scan_gen FROM root_state)"),'[(0,)]');
   const freshMutated=join(base,`fresh-mutated-${percent}`);rmSync(freshMutated,{recursive:true,force:true});await ok('init',freshMutated);await ok('scan',freshMutated);assert.equal(db(home,"SELECT path,kind,size,mtime,flags FROM entries ORDER BY path"),db(freshMutated,"SELECT path,kind,size,mtime,flags FROM entries ORDER BY path"));
   outcomes[`kill_${percent}`]=`${partial}; delete+rename committed files; ${resumed}; equal fresh crawl`;
   rmSync(replacement);writeFileSync(deleted,'x');writeFileSync(renamed,'x');
  }
  for(const point of ['building_committed','before_ready','ready_committed']){await killed('scan',home,point);outcomes[point]=await ok('scan',home);}
  const expected=db(home,"SELECT path,kind,size,mtime,flags FROM entries ORDER BY path");
  const fresh=join(base,'fresh-home');rmSync(fresh,{recursive:true,force:true});await ok('init',fresh);await ok('scan',fresh);assert.equal(db(fresh,"SELECT path,kind,size,mtime,flags FROM entries ORDER BY path"),expected);
  const mig=join(base,'migration-home');rmSync(mig,{recursive:true,force:true});await ok('init',mig);await killed('scan',mig,'migration_ddl');assert.equal(db(mig,"SELECT value FROM meta WHERE key='schema_minor'"),'[(0,)]');outcomes.migration=await ok('scan',mig);
  for(const point of ['namespace_marker_published','quarantine_main_moved','quarantine_wal_moved','quarantine_shm_moved']) {
   const namespace=join(base,`quarantine-${point}`);rmSync(namespace,{recursive:true,force:true});await ok('init',namespace);
   writeFileSync(join(namespace,'index.v3.db-wal'),'old wal');writeFileSync(join(namespace,'index.v3.db-shm'),'old shm');
   await killed('quarantine',namespace,point);outcomes[point+'_quarantine']=await ok('init',namespace);assert.equal(db(namespace,'SELECT count(*) FROM entries'),'[(0,)]');assert.equal(existsSync(join(namespace,'quarantine-3.json')),false);
  }
  for(const point of ['namespace_marker_published','gc_main_removed','gc_wal_removed','gc_shm_removed']) {
   const namespace=join(base,`gc-${point}`);rmSync(namespace,{recursive:true,force:true});await ok('gc-init',namespace);await killed('gc',namespace,point);outcomes[point+'_gc']=await ok('reuse-old',namespace);
  }
  const policy=join(base,'policy-home');rmSync(policy,{recursive:true,force:true});await ok('init',policy);db(policy,"UPDATE meta SET value='historical' WHERE key='policy_hash'");await killed('scan',policy,'policy_invalidated');assert.equal(db(policy,"SELECT status FROM root_state"),"[('dirty',)]");outcomes.policy_reconciliation=await ok('scan',policy);

  const first=join(base,'first-home');rmSync(first,{recursive:true,force:true});await killed('identity',first,'identity_synced');const x=await ok('identity',first);assert.equal(x.length,32);
  const simultaneous=join(base,'simultaneous');rmSync(simultaneous,{recursive:true,force:true});mkdirSync(simultaneous);for(let n=0;n<100;n++){const pair=join(simultaneous,`pair-${n}`);const a=start('identity',pair),b=start('identity',pair);const ar=await a.done,br=await b.done;assert.equal(ar.code,0,ar.text);assert.equal(br.code,0,br.text);assert.equal(ar.text,br.text);}outcomes.concurrent_identity='100 concurrent fresh-home pairs: one identity per pair';
  const heldMarker=join(base,'held');rmSync(heldMarker,{force:true});const held=start('hold',fresh,{KUNTU_FAULT_MARKER:heldMarker});await marker(heldMarker,held);
  const denied=await start('quarantine',fresh).done;assert.notEqual(denied.code,0);held.child.kill('SIGKILL');await held.done;
  const held2Marker=join(base,'held2');rmSync(held2Marker,{force:true});const held2=start('hold',fresh,{KUNTU_FAULT_MARKER:held2Marker});await marker(held2Marker,held2);
  const poll=start('poll',fresh);await Bun.sleep(500);const released=Date.now();held2.child.kill('SIGKILL');await held2.done;const takeover=await poll.done;assert.equal(takeover.code,0,takeover.text);const measured=Date.now()-released;assert.ok(measured<=6000);outcomes.takeover=`${measured} ms after kill; ${takeover.text.trim()}`;
  const execText=await ok('exec-holder',fresh);const pid=Number(/child_pid=(\d+)/.exec(execText)?.[1]);assert.ok(pid>0);orphaned.add(pid);process.kill(pid,0);assert.match(await ok('skew-writer',fresh),/acquired=true/);process.kill(pid,0);process.kill(pid,'SIGKILL');orphaned.delete(pid);outcomes.exec='child alive before and after a second process reacquired the writer lease';
  const source=join(base,'v2-source');rmSync(source,{recursive:true,force:true});await ok('create-v2',source);
  const files=['source.v2.db','source.v2.db-wal','source.v2.db-shm'];const hashes=()=>files.map(f=>{const p=join(source,f);return existsSync(p)?createHash('sha256').update(readFileSync(p)).digest('hex'):null});const before=hashes();
  const imported=join(base,'imported');rmSync(imported,{recursive:true,force:true});const importedResult=spawnSync(resolve(target,'release/kfs'),['store','import',join(source,'source.v2.db')],{env:{...process.env,KUNTU_HOME:imported},encoding:'utf8'});assert.equal(importedResult.status,0,importedResult.stderr);assert.deepEqual(hashes(),before);assert.equal(db(imported,'SELECT count(*) FROM entries'),'[(200001,)]');outcomes.import=importedResult.stdout.trim();
  const scopedRoot=join(base,'scoped-files');mkdirSync(join(scopedRoot,'node_modules'),{recursive:true});writeFileSync(join(scopedRoot,'visible-file.md'),'x');writeFileSync(join(scopedRoot,'.hidden-file.md'),'x');writeFileSync(join(scopedRoot,'node_modules','ignored-file.md'),'x');const scopedHome=join(base,'scoped-home');rmSync(scopedHome,{recursive:true,force:true});await ok('init-permissive',scopedHome,{KUNTU_EVIDENCE_ROOT:scopedRoot});
  const scopedEnv={...process.env,KUNTU_HOME:scopedHome};const rebuild=spawnSync(resolve(target,'release/kfs'),['index','rebuild','--root',scopedRoot],{env:scopedEnv,encoding:'utf8'});assert.equal(rebuild.status,0,rebuild.stderr);const searched=spawnSync(resolve(target,'release/kfs'),['search','file','--root',scopedRoot,'--provider','sqlite','--include-hidden','--include-ignored','--json'],{env:scopedEnv,encoding:'utf8'});assert.equal(searched.status,0,searched.stderr);assert.ok(searched.stdout.includes('visible-file'));assert.ok(!searched.stdout.includes('.hidden-file')&&!searched.stdout.includes('node_modules'));outcomes.cli_scope='strict CLI excludes union rows and supplied query flags';
  writeFileSync(join(base,'crash.json'),JSON.stringify(outcomes,null,2));console.log(JSON.stringify(outcomes,null,2));
 } else if(mode==='stress') {
  const stressRoot=join(fixtureBase,'root-1m');mkdirSync(stressRoot,{recursive:true});
  for(let n=0;n<1_000_000;n++){
   const p=join(stressRoot,`file-${String(n).padStart(6,'0')}.md`);if(!existsSync(p))writeFileSync(p,'x');
   if(n%100_000===99_999)console.log(`M2_STRESS_FIXTURE files=${n+1}`);
  }
  const eventsPath=join(base,'stress-events.jsonl'),donePath=join(base,'sampling-done');
  rmSync(eventsPath,{force:true});rmSync(donePath,{force:true});
  const extra={KUNTU_EVIDENCE_ROOT:stressRoot,KUNTU_EVIDENCE_EVENTS:eventsPath,KUNTU_SAMPLING_DONE:donePath};
  const home=join(base,'stress-home');rmSync(home,{recursive:true,force:true});await ok('init',home,extra);
  const loadBefore=machineLoad();
  const started=Date.now();const writers=[start('writer',home,extra),start('writer',home,extra)];
  const barrierDeadline=Date.now()+600_000;
  let firstCommit:WriterEvent|undefined;
  while(!(firstCommit=readEvents(eventsPath).find(e=>e.phase==='batch_commit'&&Number(e.details?.entries)>0))){
   assert.ok(writers.some(p=>p.child.exitCode===null),'writers exited before a populated batch commit');
   assert.ok(Date.now()<barrierDeadline,'populated batch barrier timed out');await Bun.sleep(20);
  }
  console.log(`M2_STRESS_POPULATED_BARRIER ${JSON.stringify(firstCommit)}`);
  const readers=Array.from({length:8},()=>start('reader',home,extra));
  const loadSamples:{at_us:number,load:string}[]=[];
  const monitor=setInterval(()=>{
   const events=readEvents(eventsPath),sample={at_us:Date.now()*1000,load:machineLoad()};loadSamples.push(sample);
   console.log(`M2_STRESS_PROGRESS ${JSON.stringify({...sample,events:events.length,last:events.at(-1)})}`);
  },30_000);
  let writerResults:{code:number|null,text:string}[];
  try{writerResults=await Promise.all(writers.map(p=>p.done));}finally{clearInterval(monitor);}
  for(const r of writerResults)assert.equal(r.code,0,r.text);
  writeFileSync(donePath,'writers joined');
  const readerResults=await Promise.all(readers.map(p=>p.done));for(const r of readerResults)assert.equal(r.code,0,r.text);
  const metrics=readerResults.map(r=>JSON.parse(r.text) as ReaderMetric);
  const events=readEvents(eventsPath);
  const overlap=metrics.map(m=>Object.fromEntries(['batch_commit','checkpoint','cleanup'].map(phase=>[phase,events.filter(e=>e.phase===phase&&e.start_us<=m.sampling_end_us!&&e.end_us>=m.sampling_start_us!).length])));
  const outliers=metrics.flatMap((m,reader)=>(m.outliers??[]).map(o=>({...o,reader,overlapping_phases:events.filter(e=>e.start_us<=Number(o.timestamp_us)+Number(o.wall_us)&&e.end_us>=Number(o.timestamp_us)).map(e=>e.phase)})));
  const report={root_entries:1_000_001,probe:'alternating populated entry-rowid point and 100-row id range; native reader floors',
   seconds:(Date.now()-started)/1000,load_before:loadBefore,load_after:machineLoad(),load_samples:loadSamples,
   barrier:firstCommit,checkpoint_note:'native 1000-page autocheckpoint remains enabled; separately recorded explicit PASSIVE checkpoints follow populated batches',
   storage_note:'read/pread/fstat syscall wall time includes scheduling; mmap/cache accesses do not issue these syscalls',
   writers:writerResults.map(r=>r.text.trim()),readers:metrics,overlap,outliers,writer_events:events,integrity:db(home,'PRAGMA integrity_check')};
  assert.equal(report.integrity,"[('ok',)]");writeFileSync(join(base,'stress.json'),JSON.stringify(report,null,2));
  console.log(JSON.stringify({...report,readers:metrics.map(({outliers,...m})=>({...m,outliers:outliers?.length??0})),outliers:outliers.length,writer_events:events.length},null,2));
  assert.equal(db(home,'SELECT count(*) FROM entries'),'[(1000001,)]');
  const misses=stressEvidence(metrics,events);assert.equal(misses.length,0,`M2_STRESS_ACCEPTANCE_MISS: ${misses.join('; ')}`);
 } else if(mode==='skew') {
  const cells:Record<string,string>={};
  const h=join(base,'skew');rmSync(h,{recursive:true,force:true});await ok('init',h,{},'0');
  for(const minor of ['0','1']){cells[`same_reader_${minor}`]=await ok('skew-reader',h,{},minor);cells[`same_writer_${minor}`]=await ok('skew-writer',h,{},minor);}
  cells.current_reader_new_minor=await ok('skew-reader',h,{},'1');cells.current_writer_new_minor=await ok('skew-writer',h,{},'1');
  cells.old_reader_new_minor=await ok('skew-reader',h,{},'0');const old=await start('skew-writer',h,{},'0').done;assert.notEqual(old.code,0);cells.old_writer_new_minor=old.text.trim();
  db(h,"UPDATE meta SET value=2 WHERE key='min_reader_minor'");const refusedBytes=readFileSync(join(h,'index.v3.db'));for(const minor of ['0','1']){const r=await start('skew-reader',h,{},minor).done;assert.notEqual(r.code,0);cells[`new_reader_floor_${minor}`]=r.text.trim();assert.deepEqual(readFileSync(join(h,'index.v3.db')),refusedBytes,'refused reader preserves database bytes');}
  const epoch4=join(h,'index.v4.db');writeFileSync(epoch4,'future epoch sentinel');const original=readFileSync(epoch4);const cli=spawnSync(resolve(target,'release/kfs'),['store','gc'],{env:{...process.env,KUNTU_HOME:h},encoding:'utf8'});assert.equal(cli.status,0,cli.stderr);assert.deepEqual(readFileSync(epoch4),original);cells.other_epoch='future sentinel unchanged';
  writeFileSync(join(base,'skew.json'),JSON.stringify(cells,null,2));console.log(JSON.stringify(cells,null,2));
 } else throw new Error(`Unknown mode ${mode}`);
} finally {
 for(const pid of orphaned){try{process.kill(pid,'SIGKILL')}catch{}}
 const remaining=[...live];
 await Promise.all(remaining.map(child=>new Promise<void>(resolve=>{child.once('close',()=>resolve());child.kill('SIGKILL');})));
}
