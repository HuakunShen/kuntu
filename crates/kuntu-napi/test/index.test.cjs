// End-to-end tests for the local NAPI package consumed from Node/Electron runtimes.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { test } = require("node:test");

const { FileSearchIndex } = require("../index.js");

async function withFixture(run) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "kfs-napi-"));
  try {
    return await run(dir);
  } finally {
    await fs.rm(dir, { recursive: true, force: true });
  }
}

test("FileSearchIndex rebuilds, reports status, and searches indexed files", async () => {
  await withFixture(async (dir) => {
    const root = path.join(dir, "root");
    const src = path.join(root, "src");
    await fs.mkdir(src, { recursive: true });
    const canonicalRoot = await fs.realpath(root);
    await fs.writeFile(path.join(root, "package.json"), "{}\n");
    await fs.writeFile(path.join(src, "notes.txt"), "notes\n");

    const index = new FileSearchIndex(path.join(dir, "kfs.sqlite"));
    const rebuild = await index.rebuild([{ path: root }]);
    assert.equal(rebuild.roots, 1);
    assert.ok(rebuild.entries >= 2);
    assert.equal(rebuild.errors.length, 0);

    const status = await index.status();
    assert.equal(status.length, 1);
    assert.equal(status[0].path, canonicalRoot);
    assert.ok(status[0].entryCount >= 2);
    assert.equal(status[0].dirty, false);

    const outcome = await index.search({
      roots: [{ path: root }],
      query: "package json",
      limit: 5,
    });

    assert.ok(outcome.candidateCount >= 1);
    assert.equal(outcome.results[0].path, path.join(canonicalRoot, "package.json"));
    assert.equal(outcome.results[0].provider, "sqlite");
    assert.ok(outcome.results[0].score > 0);
    assert.ok(outcome.results[0].matches.includes("BasenameToken"));
  });
});

test("FileSearchIndex refresh updates indexed results", async () => {
  await withFixture(async (dir) => {
    const root = path.join(dir, "root");
    await fs.mkdir(root, { recursive: true });
    const canonicalRoot = await fs.realpath(root);

    const index = new FileSearchIndex(path.join(dir, "kfs.sqlite"));
    await index.rebuild([{ path: root }]);

    await fs.writeFile(path.join(root, "readme.md"), "hello\n");
    const refresh = await index.refresh([{ path: root }]);
    assert.equal(refresh.roots, 1);
    assert.equal(refresh.inserted, 1);
    assert.equal(refresh.errors.length, 0);

    const outcome = await index.search({
      roots: [{ path: root }],
      query: "readme",
      extensions: ["md"],
      limit: 5,
    });

    assert.equal(outcome.results.length, 1);
    assert.equal(outcome.results[0].path, path.join(canonicalRoot, "readme.md"));
  });
});

test("openReadOnly enforces access on rebuild refresh repair and registration", async () => {
  await withFixture(async (dir) => {
    const home=path.join(dir,"shared");
    const root=path.join(dir,"files");await fs.mkdir(root);await fs.writeFile(path.join(root,"visible-file.md"),"x");
    const writer=FileSearchIndex.openShared(home,"strict");writer.registerRoots([{path:root}]);
    const reader=FileSearchIndex.openReadOnly(home,"strict");
    assert.equal(reader.mode(),"shared:read-only");
    for(const method of ["rebuild","refresh","repair"])await assert.rejects(reader[method]([{path:root}]),/read-only/);
    assert.throws(()=>reader.registerRoots([{path:root}]),/read-only/);
    assert.throws(()=>reader.requestRebuild([{path:root}]),/read-only/);
    await assert.rejects(reader.search({roots:[],query:""}),/scope/);
    await assert.rejects(reader.search({roots:[{path:path.join(dir,"foreign")}],query:"file"}),/scope/);
    const rows=await reader.status();assert.equal(rows.length,1);writer.close();reader.close();
  });
});

test("strict consumer ignores permissive indexing union and supplied query flags", async () => {
  await withFixture(async (dir) => {
    const home=path.join(dir,"shared");const root=path.join(dir,"files");await fs.mkdir(path.join(root,"node_modules"),{recursive:true});
    await fs.writeFile(path.join(root,"visible-file.md"),"x");await fs.writeFile(path.join(root,".hidden-file.md"),"x");await fs.writeFile(path.join(root,"node_modules","ignored-file.md"),"x");
    const strict=FileSearchIndex.openShared(home,"strict");const permissive=FileSearchIndex.openShared(home,"permissive");
    try {
      strict.registerRoots([{path:root}]);permissive.registerRoots([{path:root,includeHidden:true,includeIgnored:true}]);
      const deadline=Date.now()+6000;while(true){const rows=await permissive.status();if(rows[0]?.status==="ready"&&rows[0].entryCount>=4)break;assert.ok(Date.now()<deadline);await new Promise(r=>setTimeout(r,10));}
      const request={roots:[{path:root,includeHidden:true,includeIgnored:true}],query:"file",includeHidden:true,includeIgnored:true};
      const denied=await strict.search(request);assert.ok(denied.results.some(r=>r.path.endsWith("visible-file.md")));assert.ok(denied.results.every(r=>!r.path.includes(".hidden-file")&&!r.path.includes("node_modules")));
      const allowed=await permissive.search(request);assert.ok(allowed.results.some(r=>r.path.includes(".hidden-file")));assert.ok(allowed.results.some(r=>r.path.includes("node_modules")));
    } finally {strict.close();permissive.close();}
  });
});


test("openClient registers and queues without starting an indexer", async () => {
 await withFixture(async dir => {
  const home=path.join(dir,"shared"),root=path.join(dir,"files");await fs.mkdir(root);
  const client=FileSearchIndex.openClient(home,"client");
  try {
   client.registerRoots([{path:root}]);assert.equal(typeof client.requestRebuild([{path:root}]),"string");
   await new Promise(r=>setTimeout(r,150));assert.equal((await client.status())[0].status,"dirty");
   assert.equal((await fs.readdir(path.join(home,"locks"))).includes("indexer.lock"),false);
  } finally {client.close();}
 });
});
