// Independent Node host implementing the shared-store consumer contract.
const assert = require('node:assert/strict');
const readline = require('node:readline');
const {FileSearchIndex} = require('../crates/kuntu-napi/index.js');
const [home,root,role] = process.argv.slice(2);
assert.ok(home && root && ['read-only','writer'].includes(role));
const consumer = role==='read-only' ? 'xross-smoke' : 'kunkun-smoke';
const registration = FileSearchIndex.openClient(home,consumer);
registration.registerRoots([{path:root}]); registration.close();
const index = role==='read-only' ? FileSearchIndex.openReadOnly(home,consumer) : FileSearchIndex.openShared(home,consumer);
const request = {roots:[{path:root}],query:'visible file'};
const output = value => process.stdout.write(JSON.stringify(value)+'\n');
output({ready:true,pid:process.pid,role,mode:index.mode()});
const lines = readline.createInterface({input:process.stdin});
(async () => {
 try {
  for await (const line of lines) {
   const command=JSON.parse(line);assert.ok(command && typeof command==='object');
   if(command.op==='close') break;
   if(command.op==='read') {
    const found=await index.search(request);assert.ok(found.results.some(row=>row.path.endsWith('visible-file.md')));
    await assert.rejects(index.search({roots:[],query:''}),/scope/);
    await assert.rejects(index.search({roots:[{path:root+'/foreign'}],query:'file'}),/scope/);
    const strict=await index.search({roots:[{path:root,includeHidden:true,includeIgnored:true}],query:'file',includeHidden:true,includeIgnored:true});
    assert.ok(strict.results.every(row=>!row.path.includes('.hidden-file')&&!row.path.includes('node_modules')));
    if(role==='read-only') {
     for(const method of ['rebuild','refresh','repair']) await assert.rejects(index[method]([{path:root}]),/read-only/);
     assert.throws(()=>index.registerRoots([{path:root}]),/read-only/);
     assert.throws(()=>index.requestRebuild([{path:root}]),/read-only/);
    }
    output({read:true,pid:process.pid,role,holder:index.writerLeaseHeld(),matches:found.results.length});
   } else if(command.op==='queue') {
    assert.equal(role,'writer');output({queued:index.requestRebuild([{path:root}]),holder:index.writerLeaseHeld()});
   } else throw new Error('Unknown host command.');
  }
 } finally { index.close(); lines.close(); }
})().catch(error=>{console.error(error);process.exitCode=1;lines.close();process.stdin.destroy();});
