import assert from 'node:assert/strict';
import {mkdtemp,mkdir,writeFile,rm,realpath} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {createServer,loadConfigFromFile} from 'vite';
const original=process.cwd();
const root=await realpath(await mkdtemp(join(tmpdir(),'magi-watch-')));
let server;
const events=[];
try {
  await mkdir(join(root,"src"));
  await writeFile(join(root,"src/probe.ts"),"export const probe=0;");
  const {config}=await loadConfigFromFile({command:'serve',mode:'development'},resolve(original,'vite.config.ts'));
  config.server.watch.ignored=config.server.watch.ignored.map(value=>value.replace(original,root));
  server=await createServer({...config,root,configFile:false,plugins:[],server:{...config.server,host:'127.0.0.1',port:0,strictPort:false,watch:{...config.server.watch,usePolling:true,interval:50}}});
  server.watcher.on('all',(kind,file)=>events.push({kind,file}));
  await server.listen();
  server.watcher.add(root);
  await new Promise(resolve=>setTimeout(resolve,300));
  for(const directory of ['.local/provider-build/source','tmp/codex-acp-build/source','src-tauri/target/release']) {
    await mkdir(join(root,directory),{recursive:true});
    await writeFile(join(root,directory,'tsconfig.json'),'{}');
    await writeFile(join(root,directory,'staging_runtime.py'),'# generated');
  }

  await writeFile(join(root,'src/probe.ts'),'export const probe=1;');
  await new Promise(resolve=>setTimeout(resolve,700));
  assert(events.some(event=>event.file.endsWith('/src/probe.ts')),'actual source must remain watched');
  assert(!events.some(event=>event.file.endsWith('tsconfig.json')||event.file.endsWith('staging_runtime.py')),'generated build changes must not reach watcher');
  console.log('PASS generated tsconfig and staging activity ignored; actual source remains watched');
} finally {
  process.chdir(original);
  try {await server?.close();} finally {await rm(root,{recursive:true,force:true});}
}
