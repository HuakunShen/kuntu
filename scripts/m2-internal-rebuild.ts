/** Internal-SSD rebuild acceptance; external-volume evidence cannot satisfy it. */
import {spawnSync} from 'node:child_process';
import {writeFileSync,mkdirSync} from 'node:fs';
import {join,resolve} from 'node:path';
import {strict as assert} from 'node:assert';
const name='internal_ssd_one_million_rebuild_within_120_seconds';
const secondsCeiling=120;
function withinCeiling(seconds:number){return seconds<=secondsCeiling;}
if(process.argv[2]==='--contracts'){
 console.log(`RUN ${name}_ceiling_contract`);
 assert.ok(withinCeiling(120));assert.equal(withinCeiling(120.001),false,'internal SSD ceiling was loosened');console.log(`PASS ${name}_ceiling_contract`);
}else{
 const artifacts=process.env.KUNTU_ACCEPTANCE_DIR??'/Volumes/Portable2TB/tmp/kuntu-m2/rev3/acceptance';mkdirSync(artifacts,{recursive:true});
 const root=process.env.KUNTU_INTERNAL_SSD_ROOT;const home=process.env.KUNTU_INTERNAL_SSD_HOME;
 if(!root||!home){
  const pending={test:name,state:'NOT RUN',ceiling_seconds:secondsCeiling,reason:'No internal-SSD fixture/home within the authorized scratch directory.'};writeFileSync(join(artifacts,'internal-rebuild.json'),JSON.stringify(pending,null,2));console.log(`M2_INTERNAL_SSD_NOT_RUN ${JSON.stringify(pending)}`);process.exitCode=2;
 }else{
  for(const path of [root,home])assert.ok(resolve(path).startsWith('/Volumes/Portable2TB/tmp/kuntu-m2/'),'scratch data must stay in the authorized directory');
  assert.equal(process.platform,'darwin');
  for(const path of [root,home]){
   const disk=spawnSync('diskutil',['info','-plist',path],{encoding:'buffer'});assert.equal(disk.status,0);
   const decoded=spawnSync('python3',['-c','import sys,plistlib; print(plistlib.loads(sys.stdin.buffer.read()).get("Internal",False))'],{input:disk.stdout,encoding:'utf8'});assert.equal(decoded.status,0);assert.equal(decoded.stdout.trim(),'True','target is not an internal SSD');
  }
  const bin=resolve(process.env.CARGO_TARGET_DIR??'target','release/examples/m2_probe');
  const init=spawnSync(bin,['init',home,root,'1'],{encoding:'utf8'});assert.equal(init.status,0,init.stderr);
  const start=performance.now();const result=spawnSync(bin,['scan',home,root,'1'],{encoding:'utf8'});const seconds=(performance.now()-start)/1000;
  assert.equal(result.status,0,result.stderr);assert.match(result.stdout,/entries=1000001\b/);
  const report={test:name,state:withinCeiling(seconds)?'PASS':'OPEN RED',seconds,ceiling_seconds:secondsCeiling,result:result.stdout.trim()};writeFileSync(join(artifacts,'internal-rebuild.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report));assert.ok(withinCeiling(seconds),`${name} M2_ACCEPTANCE_MISS ${seconds}>${secondsCeiling}`);
 }
}
