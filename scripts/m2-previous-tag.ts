/** Actual previous-tag epoch isolation; no checkout, branch or tag mutation. */
import {spawnSync} from 'node:child_process';
import {mkdirSync,rmSync,readFileSync,writeFileSync,existsSync} from 'node:fs';
import {resolve,join} from 'node:path';
import {strict as assert} from 'node:assert';
const base='/Volumes/Portable2TB/tmp/kuntu-m2/previous-tag';
const commit='a5df3559671b09f2af75fdd28ed5a4a06ab7dc56';
const src=join(base,'source');mkdirSync(src,{recursive:true});
function call(args:string[],cwd?:string,env:Record<string,string>={}){
 const r=spawnSync(args[0]!,args.slice(1),{cwd,env:{...process.env,...env},encoding:'utf8',maxBuffer:64*1024*1024});
 return {status:r.status,text:r.stdout+r.stderr};
}
function ok(args:string[],cwd?:string,env:Record<string,string>={}){const r=call(args,cwd,env);assert.equal(r.status,0,r.text);return r.text.trim();}
const archive=join(base,'previous.tar');
ok(['git','archive','--format=tar','--output',archive,commit]);ok(['tar','-xf',archive,'-C',src]);
const build=call(['cargo','build','--release','--locked','-p','kuntu-cli'],src,{CARGO_TARGET_DIR:join(base,'target')});writeFileSync(join(base,'build.log'),build.text);assert.equal(build.status,0,build.text);
const old=join(base,'target/release/kfs');
const target=process.env.CARGO_TARGET_DIR??'target';const now=resolve(target,'release/kfs'),probe=resolve(target,'release/examples/m2_probe');
const root=join(base,'files');mkdirSync(root,{recursive:true});writeFileSync(join(root,'retained-file.md'),'x');
const home=join(base,'home');rmSync(home,{recursive:true,force:true});ok([probe,'init',home,root]);
const db=join(home,'index.v2.db');ok([old,'index','rebuild','--root',root,'--db',db]);
const original=readFileSync(db);
const cells:Record<string,string>={previous_tag:commit,previous_epoch:ok([old,'index','status','--db',db])};
cells.current_private_v2=ok([now,'index','status','--db',db]);
assert.deepEqual(readFileSync(db),original,'new private compatibility must not migrate v2 main file');
cells.current_epoch=ok([probe,'scan',home,root]);
const v3=join(home,'index.v3.db');const before=readFileSync(v3);
const oldOnV3=call([old,'index','status','--db',v3]);assert.notEqual(oldOnV3.status,0);assert.match(oldOnV3.text,/schema|version|supported/i);assert.deepEqual(readFileSync(v3),before,'previous tag refusal must preserve epoch3 main bytes');cells.previous_refuses_epoch3=oldOnV3.text.trim();
cells.gc=ok([now,'store','gc'],undefined,{KUNTU_HOME:home});assert.ok(existsSync(db));assert.deepEqual(readFileSync(db),original,'unregistered previous-tag epoch remains preserved');
cells.other_epoch='epoch2 and epoch3 remain distinct; previous tag refuses user_version3; current private-v2 reader preserves source';
writeFileSync(join(base,'matrix.json'),JSON.stringify(cells,null,2));console.log(JSON.stringify(cells,null,2));console.log("PASS previous_tag_epoch_isolation");
