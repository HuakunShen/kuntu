/** macOS contract smoke: CLI, daemon and two independent NAPI host processes. */
import {spawn,spawnSync,type ChildProcess} from 'node:child_process';
import {mkdirSync,writeFileSync,readFileSync,rmSync} from 'node:fs';
import {resolve,join} from 'node:path';
import {strict as assert} from 'node:assert';
const name='macos_four_host_shared_store_consumer_contract';
const base='/Volumes/Portable2TB/tmp/kuntu-m2/rev3/consumer-smoke';
const kfs=resolve(process.env.CARGO_TARGET_DIR??'target','release/kfs');
const children=new Set<ChildProcess>();
function child(args:string[],env:Record<string,string|undefined>={}) {
 const proc=spawn(args[0]!,args.slice(1),{env:{...process.env,...env},stdio:['pipe','pipe','pipe']});children.add(proc);
 const lines:string[]=[];let pending='';let all='';
 proc.stdout?.on('data',(bytes:Buffer)=>{const text=bytes.toString();all+=text;pending+=text;const parts=pending.split('\n');pending=parts.pop()??'';lines.push(...parts);});
 proc.stderr?.on('data',(bytes:Buffer)=>{all+=bytes.toString();});
 const done=new Promise<{code:number|null,text:string}>(finish=>proc.on('close',code=>{children.delete(proc);finish({code,text:all});}));
 async function line(){const end=Date.now()+10000;while(!lines.length){assert.equal(proc.exitCode,null,all);assert.ok(Date.now()<end,`${name}: host reply timeout`);await Bun.sleep(10);}const value:unknown=JSON.parse(lines.shift()!);assert.ok(typeof value==='object'&&value!==null);return value;}
 return {proc,done,line};
}
async function send(host:ReturnType<typeof child>,op:string){host.proc.stdin?.write(JSON.stringify({op})+'\n');return host.line();}
try {
 assert.equal(process.platform,'darwin',`${name} requires macOS`);
 rmSync(base,{recursive:true,force:true});const home=join(base,'home'),root=join(base,'files');mkdirSync(join(root,'node_modules'),{recursive:true});
 for(const file of ['visible-file.md','.hidden-file.md','node_modules/ignored-file.md'])writeFileSync(join(root,file),'x');
 const env={KUNTU_HOME:home};
 const seed=spawnSync(kfs,['index','rebuild','--root',root],{env:{...process.env,...env},encoding:'utf8'});assert.equal(seed.status,0,seed.stderr);
 const identity=readFileSync(join(home,'store.json'),'utf8');
 // Keep the daemon alive until the four processes have exercised the store.
 const server=Bun.serve({hostname:'127.0.0.1',port:0,fetch:()=>new Response()});const port=server.port;server.stop(true);
 const daemon=child([kfs,'daemon','--root',root,'--addr',`127.0.0.1:${port}`,'--duration-ms','6000'],env);
 const url=`http://127.0.0.1:${port}`;const end=Date.now()+6000;
 while(true){try{const response=await fetch(url+'/health');if(response.ok)break;}catch{}assert.ok(Date.now()<end);await Bun.sleep(10);}
 // Verify the daemon actually owns the lease, rather than only being healthy.
 while(true){const text=readFileSync(join(home,'locks/indexer.lock'),'utf8');if(JSON.parse(text).pid===daemon.proc.pid)break;assert.ok(Date.now()<end);await Bun.sleep(10);}
 const reader=child(['node','scripts/m2-napi-host.cjs',home,root,'read-only']);const writer=child(['node','scripts/m2-napi-host.cjs',home,root,'writer']);
 const readyReader=await reader.line(),readyWriter=await writer.line();assert.notEqual(reader.proc.pid,writer.proc.pid);
 assert.ok('ready'in readyReader&&readyReader.ready===true&&'ready'in readyWriter&&readyWriter.ready===true);
 const read=await send(reader,'read'),writeRead=await send(writer,'read');assert.ok('holder'in writeRead&&writeRead.holder===false,'daemon must exclude the NAPI writer');
 const queued=await send(writer,'queue');assert.ok('queued'in queued&&typeof queued.queued==='string');
 const cli=child([kfs,'search','file','--root',root,'--provider','sqlite','--json'],env);assert.equal(daemon.proc.exitCode,null);assert.equal(reader.proc.exitCode,null);assert.equal(writer.proc.exitCode,null);
 const searched=await cli.done;assert.equal(searched.code,0,searched.text);assert.ok(searched.text.includes('visible-file'));assert.ok(!searched.text.includes('.hidden-file')&&!searched.text.includes('node_modules'));
 const response=await fetch(url+'/search',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({roots:[root],query:'visible file'})});assert.equal(response.status,200);const daemonSearch:unknown=await response.json();assert.ok(typeof daemonSearch==='object'&&daemonSearch!==null&&'results'in daemonSearch&&Array.isArray(daemonSearch.results)&&daemonSearch.results.length>0);
 const daemonDone=await daemon.done;assert.equal(daemonDone.code,0,daemonDone.text);const released=Date.now();
 let takeover:unknown;
 do{takeover=await send(writer,'read');assert.ok(Date.now()-released<=6000,`${name}: NAPI writer never took over`);if(typeof takeover==='object'&&takeover!==null&&'holder'in takeover&&takeover.holder===true)break;await Bun.sleep(20);}while(true);
 const takeoverMs=Date.now()-released;await send(reader,'read');assert.equal(readFileSync(join(home,'store.json'),'utf8'),identity);
 reader.proc.stdin?.write('{"op":"close"}\n');writer.proc.stdin?.write('{"op":"close"}\n');assert.equal((await reader.done).code,0);assert.equal((await writer.done).code,0);
 const report={test:name,cli:true,daemon:true,napi_read_only_pid:reader.proc.pid,napi_writer_pid:writer.proc.pid,readonly:read,writer_queued:queued,takeover_ms:takeoverMs,one_identity:true};
 writeFileSync(join(base,'result.json'),JSON.stringify(report,null,2));console.log(`PASS ${name} ${JSON.stringify(report)}`);
} catch(error){console.error(`${name} FAILED`,error);process.exitCode=1;} finally {
 await Promise.all([...children].map(proc=>new Promise<void>(done=>{proc.once('close',()=>done());proc.kill('SIGKILL');})));
}
