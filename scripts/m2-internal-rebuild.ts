/** Internal-SSD rebuild acceptance; external-volume evidence cannot satisfy it. */
import {spawnSync} from 'node:child_process';
import {writeFileSync,mkdirSync,lstatSync,realpathSync} from 'node:fs';
import {join,resolve,isAbsolute} from 'node:path';
import {strict as assert} from 'node:assert';
const name='internal_ssd_one_million_rebuild_within_120_seconds';
const secondsCeiling=120;
function withinCeiling(seconds:number){return seconds<=secondsCeiling;}
function ownerInternalPath(path:string):string {
 assert.ok(isAbsolute(path),'owner must supply an absolute internal path');
 assert.ok(!path.split('/').includes('..'),'parent traversal is not an owner path');
 const absolute=resolve(path);
 assert.ok(absolute!=='/'&&absolute!=='/Volumes'&&!absolute.startsWith('/Volumes/'),'owner path must be on the internal disk, outside /Volumes');
 return absolute;
}
function validateOwner(actual:number,expected:number):void {
 assert.equal(actual,expected,'internal path owner must match the runner owner');
}
function pathContract(){
 const test='owner_supplied_absolute_internal_paths_are_accepted';console.log(`RUN ${test}`);
 for(const path of ['/Users/owner/kuntu/fixture','/Users/owner/Kuntu Test/home','/private/var/owner/kuntu'])assert.equal(ownerInternalPath(path),path);
 for(const path of ['relative','', '~/kuntu','/','/Volumes','/Volumes/External/fixture','/Users/owner/../other'])assert.throws(()=>ownerInternalPath(path));
 console.log(`PASS ${test}`);
}
function ownerContract(){
 const test='internal_paths_require_the_runner_owner';console.log(`RUN ${test}`);
 validateOwner(501,501);assert.throws(()=>validateOwner(502,501),/owner/i);
 console.log(`PASS ${test}`);
}

if(process.argv[2]==='--path-contract'){pathContract();
}else if(process.argv[2]==='--owner-contract'){ownerContract();
}else if(process.argv[2]==='--contracts'){
 console.log(`RUN ${name}_ceiling_contract`);
 assert.ok(withinCeiling(120));assert.equal(withinCeiling(120.001),false,'internal SSD ceiling was loosened');console.log(`PASS ${name}_ceiling_contract`);pathContract();ownerContract();
}else{
 const artifacts=process.env.KUNTU_ACCEPTANCE_DIR??'/Volumes/Portable2TB/tmp/kuntu-m2impl-bench/acceptance';mkdirSync(artifacts,{recursive:true});
 const root=process.env.KUNTU_INTERNAL_SSD_ROOT;const home=process.env.KUNTU_INTERNAL_SSD_HOME;
 if(!root||!home){
  const pending={test:name,state:'NOT RUN',ceiling_seconds:secondsCeiling,reason:'Owner has not supplied existing absolute internal fixture/home paths.'};writeFileSync(join(artifacts,'internal-rebuild.json'),JSON.stringify(pending,null,2));console.log(`M2_INTERNAL_SSD_NOT_RUN ${JSON.stringify(pending)}`);process.exitCode=2;
 }else{
  const paths=[root,home].map(ownerInternalPath);
  assert.equal(process.platform,'darwin');
  for(const path of paths){
   const stat=lstatSync(path);
   assert.ok(stat.isDirectory()&&!stat.isSymbolicLink(),'owner path must be an existing directory, not a symlink');
   validateOwner(stat.uid,process.getuid!());
   const canonical=ownerInternalPath(realpathSync(path));
   const df=spawnSync('df',['-P',canonical],{encoding:'utf8'});assert.equal(df.status,0,df.stderr);const mount=df.stdout.trim().split('\n').at(-1)!.split(/\s+/).slice(5).join(' ');assert.ok(isAbsolute(mount),'cannot resolve the mount point');
   const disk=spawnSync('diskutil',['info','-plist',mount],{encoding:'buffer'});assert.equal(disk.status,0,'diskutil accepts mount points, not arbitrary folders');
   const decoded=spawnSync('python3',['-c','import sys,plistlib; print(plistlib.loads(sys.stdin.buffer.read()).get("Internal",False))'],{input:disk.stdout,encoding:'utf8'});assert.equal(decoded.status,0);assert.equal(decoded.stdout.trim(),'True','target is not an internal SSD');
  }
  const bin=resolve(process.env.CARGO_TARGET_DIR??'target','release/examples/m2_probe');
  const init=spawnSync(bin,['init',home,root,'1'],{encoding:'utf8'});assert.equal(init.status,0,init.stderr);
  const start=performance.now();const result=spawnSync(bin,['scan',home,root,'1'],{encoding:'utf8'});const seconds=(performance.now()-start)/1000;
  assert.equal(result.status,0,result.stderr);assert.match(result.stdout,/entries=1000001\b/);
  const report={test:name,state:withinCeiling(seconds)?'PASS':'OPEN RED',seconds,ceiling_seconds:secondsCeiling,result:result.stdout.trim()};writeFileSync(join(artifacts,'internal-rebuild.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report));assert.ok(withinCeiling(seconds),`${name} M2_ACCEPTANCE_MISS ${seconds}>${secondsCeiling}`);
 }
}
