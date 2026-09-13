import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const source=readFileSync(new URL('../legacy-web/app.js',import.meta.url),'utf8');
function load(context,name) {
  const start=new RegExp(`^(?:async )?function ${name}\\(`,'m').exec(source);
  assert.ok(start,name);
  const end=source.indexOf('\n}\n',start.index);
  vm.runInContext(source.slice(start.index,end+2),context);
  return context[name];
}
const path='/api/media/'+'a'.repeat(32);
function setup(config={backend:'rust',media_lazy:true}) {
  const context=vm.createContext({AgentHubCapabilities:{config,allows:()=>true},HUB_MODE:true,
    URL,AbortController,TextDecoder,Uint8Array,mediaDiagnostics:new Map(),mediaDiagnosticActive:0,
    setTimeout:()=>1,clearTimeout:()=>{},appUrl:x=>x,esc:x=>String(x).replaceAll('"','&quot;')});
  for(const name of ['lazyMediaEnabled','safeMediaSrc','imageHtml','diagnoseMedia']) load(context,name);
  return context;
}

test('lazy handling is exact Rust capability gated; Python markup remains unchanged',()=>{
  for(const config of [{},{backend:'python',media_lazy:true},{backend:'rust',media_lazy:1},
    {backend:'rust',media_lazy:true}]) {
    const c=setup(config),html=c.imageHtml({src:path,alt:'image',lazy:true});
    assert.equal(html.includes('data-media-lazy="true"'),config.backend==='rust'&&config.media_lazy===true);
    assert.match(html,/loading="lazy"/);assert.doesNotMatch(html,/width=|height=|data:image/);
  }
  const c=setup();
  for(const value of ['https://example.invalid/x','file:///x','/native/x','data:image/png;base64,eA==',
    '/api/media/'+'A'.repeat(32),path+'?x=1','/api/nodes/'+'b'.repeat(32)+path]) {
    assert.equal(c.safeMediaSrc(value),'');
  }
  assert.equal(setup({}).safeMediaSrc('https://example.invalid/x'),'https://example.invalid/x');
});

test('diagnostic deduplicates token, limits concurrency without queue, and bounds retained results',async()=>{
  const c=setup();let calls=0;const resolves=[];
  c.fetch=()=>{calls++;return new Promise(resolve=>resolves.push(resolve));};
  const response=()=>({ok:false,status:503,headers:new Headers({'content-type':'application/json'}),
    body:new Response(JSON.stringify({error:'bounded busy'})).body});
  const jobs=[];
  for(let i=0;i<4;i++) jobs.push(c.diagnoseMedia('/api/media/'+String(i).repeat(32)));
  const duplicate=c.diagnoseMedia('/api/media/'+'0'.repeat(32));
  assert.match((await c.diagnoseMedia(path)).message,/繁忙/);assert.equal(calls,4);
  c.mediaDiagnostics.clear();
  assert.match((await c.diagnoseMedia(path)).message,/繁忙/);assert.equal(calls,4);
  for(const resolve of resolves) resolve(response());
  await Promise.all([...jobs,duplicate]);assert.equal(c.mediaDiagnosticActive,0);
  c.fetch=async()=>{calls++;return response();};
  assert.equal((await c.diagnoseMedia('/api/media/'+'0'.repeat(32))).status,503);assert.equal(calls,5);
  await c.diagnoseMedia('/api/media/'+'0'.repeat(32));assert.equal(calls,5);
  c.fetch=async()=>response();
  for(let i=10;i<150;i++) await c.diagnoseMedia('/api/media/'+i.toString(16).padStart(32,'0'));
  assert.equal(c.mediaDiagnostics.size,128);
});

test('diagnostic cancels successful image without reading and rejects oversized error bodies',async()=>{
  for(const variant of ['image','large-header','large-stream']) {
    const c=setup();let cancelled=0,reads=0;
    const headers=new Headers({'content-type':variant==='image'?'image/png':'application/json'});
    if(variant==='large-header') headers.set('content-length','99999');
    c.fetch=async()=>({ok:variant==='image',status:variant==='image'?200:422,headers,body:{
      cancel:async()=>{cancelled++;},getReader:()=>({read:async()=>{reads++;return{value:new Uint8Array(4097),done:false};},
        cancel:async()=>{cancelled++;},releaseLock(){}})}});
    const result=await c.diagnoseMedia(path);
    assert.equal(result.status,variant==='image'?200:422);assert.equal(cancelled,1);
    assert.equal(reads,variant==='large-stream'?1:0);
  }
});

test('diagnostic network and deadline failures are fixed messages and never automatic retries',async()=>{
  for(const name of ['AbortError','TypeError']) {
    const c=setup();let calls=0;
    c.fetch=async()=>{calls++;throw Object.assign(new Error('private path/secret'),{name});};
    const result=await c.diagnoseMedia(path);await c.diagnoseMedia(path);
    assert.doesNotMatch(result.message,/private|secret/);assert.equal(calls,1);
  }
});

test('explicit image-window reload rejects newer observations and does not require a gap',async()=>{
  for(const changed of [null,'msgs','version','end','anchor','activity','meta','prompt','partial']) {
    const c=setup(),entry={msgs:[],version:{},end:1,anchor:'old',activity:null,meta:{},prompt:null,partial:null};
    const cache=new Map([['u',entry]]);let resolve,applied=0;
    Object.assign(c,{cache,viewKey:x=>x,historyPageRequests:new Map(),SYNC_STALL_MS:10000,
      fetchMessages:()=>new Promise(done=>{resolve=done;}),applyDiff:async()=>{applied++;}});
    load(c,'reloadMediaSession');
    const notice={},button={},task=c.reloadMediaSession({uid:'u',agent:null,current:()=>true},button,notice);
    if(changed) entry[changed]={newer:true};
    resolve({data:{reset:true,meta:{},version:{},messages:[]},bytes:1});await task;
    assert.equal(applied,changed?0:1);assert.equal(c.historyPageRequests.size,0);
    if(changed) assert.match(notice.textContent,/实时历史已更新/);
  }
});

test('all Rust reset renders reconcile the published entry; Python keeps its old render options',async()=>{
  for(const backend of ['rust','python']) {
    const c=setup({backend}),cache=new Map([['u:a',{msgs:[],end:1}]]),rendered=[];
    Object.assign(c,{cache,S:{sel:'u',agent:'a',cursors:new Map()},viewKey:(uid,agent)=>uid+':'+agent,
      migrationReadPaused:()=>false,markInterruptedTurn:()=>{},cachePut:(key,e)=>cache.set(key,e),
      renderSession:async(...args)=>rendered.push(args)});
    load(c,'applyDiff');
    await c.applyDiff('u',{reset:true,meta:{uid:'u',agent_id:'a'},messages:[{role:'user',text:'old'}],
      version:{head:'h'},end:2,anchor:'anchor'},100,'a');
    assert.equal(rendered.length,1);
    if(backend==='rust') assert.equal(rendered[0][3].historyPageEntry,cache.get('u:a'));
    else assert.equal(Object.keys(rendered[0][3]).length,0);
  }
});
