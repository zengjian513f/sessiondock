import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../legacy-web/app.js', import.meta.url), 'utf8');
function load(context, name) {
  const match = new RegExp(`^(?:async )?function ${name}\\(`, 'm').exec(source);
  assert.ok(match, name);
  const end = source.indexOf('\n}\n', match.index);
  vm.runInContext(source.slice(match.index, end + 2), context);
  return context[name];
}
const A = 'a'.repeat(32), B = 'b'.repeat(32);
function element(tag, className, textContent = '') {
  return {tag, className, textContent, disabled: false, isConnected: true, children: [],
    setAttribute() {}, getBoundingClientRect() {return {top:120};},
    appendChild(item) {this.children.push(item); item.parentElement = this;},
    querySelector(selector) {return this.children.find(item => '.' + item.className === selector) || null;},
    remove() {this.isConnected = false;}};
}
function fixture(chain=false) {
  const entry = {msgs:['head','tail0','tail1'],partial:{head:1,tail:2,omitted:4,cursor:A},
    meta:{uid:'codex:fixture'},bytes:100,total:7,end:100,anchor:'old',version:{head:'old-head'},activity:null};
  const cache = new Map([['codex:fixture',entry]]), rendered=[];
  const gap = element('div','history-gap'), button=element('button','history-gap-load'); gap.appendChild(button);
  const context = vm.createContext({AgentHubCapabilities:{config:{backend:'rust',history_pages:true}},
    S:{sel:'codex:fixture',agent:null,cursors:new Map([['codex:fixture',{end:100,head:'old-head',anchor:'old'}]])},
    cache, inflight:{}, viewKey:(uid,agent)=>agent?`${uid}:${agent}`:uid,
    historyPageRequests:new Map(), AbortController, SYNC_STALL_MS:20000, HISTORY_PAGE_CHAIN:chain, HISTORY_PAGE_MAX_EVENTS:10000,
    setTimeout:()=>1,clearTimeout:()=>{},trimCache:()=>{},el:element,
    $:selector=>selector==='#msgs .history-gap-load'?button:({scrollTop:300}),renderSession:async(...args)=>rendered.push(args),restoreHistoryPageScroll:()=>{}});
  for (const name of ['historyPagesEnabled','currentHistoryPage','currentHistoryPageEntry','validateHistoryPage','historyPageFailure','loadHistoryPage']) load(context,name);
  return {context,entry,cache,button,gap,rendered};
}
const message = text => ({role:'user',text});
function page() {return {data:{messages:['page0','page1'].map(message),page:{cursor:A,next:B,start:1,end:3,stop:5,remaining:2}},bytes:20};}

test('only exact Rust history_pages capability selects pages; Python keeps full-history behavior', () => {
  for (const config of [{},{backend:'python'},{backend:'rust'},{backend:'rust',history_pages:1},
    {backend:'rust',history_pages:false},{backend:'rust',history_pages:true}]) {
    const calls=[];
    const context=vm.createContext({AgentHubCapabilities:{config},el:element,
      loadHistoryPage:()=>calls.push('page'),loadFullHistory:()=>calls.push('full')});
    load(context,'historyPagesEnabled'); const gap=load(context,'historyGapNode')({uid:'u',agent:null,omitted:800});
    gap.children[0].onclick();
    assert.deepEqual(calls,[config.backend==='rust'&&config.history_pages===true?'page':'full']);
  }
});

test('page validation rejects wrong scope cursor, non-progress, noninteger and inconsistent bounds', () => {
  const {context,entry}=fixture(), validate=context.validateHistoryPage;
  assert.equal(validate(page().data,entry.partial,A).end,3);
  for (const change of [{cursor:B},{start:0},{end:4},{stop:6},{remaining:1},{next:null},
    {end:1},{start:1.5},{end:Infinity},{next:A}]) {
    const data=page().data;Object.assign(data.page,change);
    assert.throws(()=>validate(data,entry.partial,A));
  }
  const large=page().data;large.messages=Array(201).fill('x');assert.throws(()=>validate(large,entry.partial,A));
  for (const invalid of [null,'text',{},[],{role:'user',text:null},{role:'user',text:'ok',media:[null]}]) {
    const data=page().data;data.messages[0]=invalid;assert.throws(()=>validate(data,entry.partial,A));
  }
});

test('a delayed page retains an SSE-advanced tail, live cursor, metadata and total', async () => {
  const {context,entry,button,rendered}=fixture();let resolve;
  context.fetchHistoryPage=()=>new Promise(done=>{resolve=done;});
  const task=context.loadHistoryPage('codex:fixture',null,button);
  entry.msgs=entry.msgs.concat('live');entry.end=999;entry.anchor='live-anchor';entry.total=8;
  entry.version={head:'live-head'};
  const live={end:999,head:'live-head',anchor:'live-anchor'};context.S.cursors.set('codex:fixture',live);
  resolve(page());await task;
  assert.deepEqual(Array.from(entry.msgs,m=>m.text||m),['head','page0','page1','tail0','tail1','live']);
  assert.equal(entry.partial.head,3);assert.equal(entry.partial.omitted,2);assert.equal(entry.partial.cursor,B);
  assert.equal(entry.end,999);assert.equal(entry.anchor,'live-anchor');assert.equal(entry.total,8);
  assert.equal(context.S.cursors.get('codex:fixture'),live);
  assert.equal(rendered.length,1);assert.equal(rendered[0][3].startWatch,false);assert.equal(rendered[0][3].historyPageEntry,entry);
});

test('view changes, same-view reset and changed page cursor discard delayed responses', async () => {
  for (const mutate of [state=>{state.context.S.sel='other';},state=>{state.context.inflight={};},
    state=>{state.cache.set('codex:fixture',{...state.entry});},state=>{state.entry.partial.cursor=B;}]) {
    const state=fixture();let resolve;state.context.fetchHistoryPage=()=>new Promise(done=>{resolve=done;});
    const task=state.context.loadHistoryPage('codex:fixture',null,state.button);mutate(state);resolve(page());await task;
    assert.deepEqual(state.entry.msgs,['head','tail0','tail1']);assert.equal(state.rendered.length,0);
  }
});

test('duplicate clicks share no extra request and a completed gap does not touch live cursor', async () => {
  const {context,entry,button}=fixture();let calls=0,resolve;
  context.fetchHistoryPage=()=>{calls++;return new Promise(done=>{resolve=done;});};
  const task=context.loadHistoryPage('codex:fixture',null,button);
  await context.loadHistoryPage('codex:fixture',null,button);assert.equal(calls,1);
  const reply=page();reply.data.messages=['p0','p1','p2','p3'].map(message);Object.assign(reply.data.page,{end:5,remaining:0,next:null});
  resolve(reply);await task;assert.equal(entry.partial,null);
  assert.equal(context.S.cursors.get('codex:fixture').end,100);
});

test('404/409/410 preserve the snapshot and offer only an explicit window reload', async () => {
  for (const status of [404,409,410]) {
    const {context,entry,button,gap}=fixture();let reloads=0;
    context.reloadHistoryWindow=()=>{reloads++;};
    context.fetchHistoryPage=async()=>{throw Object.assign(new Error(`specific ${status}`),{status});};
    await context.loadHistoryPage('codex:fixture',null,button);
    assert.deepEqual(entry.msgs,['head','tail0','tail1']);assert.equal(entry.partial.cursor,A);
    assert.match(gap.querySelector('.history-page-error').textContent,new RegExp(`specific ${status}`));
    assert.equal(reloads,0);gap.querySelector('.history-gap-reload').onclick();assert.equal(reloads,1);
  }
});

test('one click chains pages until the gap is filled, shows progress and renders once', async () => {
  const {context,entry,button,rendered}=fixture(true);
  const cursors=[];
  context.fetchHistoryPage=async(uid,agent,cursor)=>{
    cursors.push(cursor);
    if (cursor===A) return page();
    const last=page();last.data.messages=['page2','page3'].map(message);
    Object.assign(last.data.page,{cursor:B,next:null,start:3,end:5,remaining:0});
    return last;
  };
  const task=context.loadHistoryPage('codex:fixture',null,button);
  assert.equal(button.disabled,false,'the running button stays clickable to abort');
  assert.match(button.textContent,/正在加载历史… 0 \/ 4 条/);
  await task;
  assert.deepEqual(cursors,[A,B]);
  assert.deepEqual(Array.from(entry.msgs,m=>m.text||m),['head','page0','page1','page2','page3','tail0','tail1']);
  assert.equal(entry.partial,null);
  assert.equal(rendered.length,1,'pages are rendered once at the end, not per page');
  assert.equal(context.historyPageRequests.size,0);
});

test('a second click aborts the chain, keeps the pages already read and renders them', async () => {
  const {context,entry,button,rendered,gap}=fixture(true);
  let resolveSecond;
  context.fetchHistoryPage=(uid,agent,cursor,signal)=>cursor===A?Promise.resolve(page())
    :new Promise((done,fail)=>{resolveSecond=done;signal.addEventListener('abort',()=>fail(Object.assign(new Error('aborted'),{name:'AbortError'})));});
  const task=context.loadHistoryPage('codex:fixture',null,button);
  await new Promise(done=>setImmediate(done));
  assert.match(button.textContent,/2 \/ 4 条 · 点击中止/);
  await context.loadHistoryPage('codex:fixture',null,button);   // second click = abort
  await task;
  assert.deepEqual(Array.from(entry.msgs,m=>m.text||m),['head','page0','page1','tail0','tail1']);
  assert.equal(entry.partial.cursor,B);assert.equal(entry.partial.omitted,2);
  assert.equal(rendered.length,1);
  assert.equal(gap.querySelector('.history-page-error'),null,'an abort is not an error');
  assert.equal(context.historyPageRequests.size,0);
});

test('a failure after progress renders the pages read so far and reports on the fresh gap', async () => {
  const {context,entry,button,rendered,gap}=fixture(true);
  context.reloadHistoryWindow=()=>{};
  context.fetchHistoryPage=async(uid,agent,cursor)=>{if(cursor===A)return page();throw Object.assign(new Error('specific 410'),{status:410});};
  await context.loadHistoryPage('codex:fixture',null,button);
  assert.deepEqual(Array.from(entry.msgs,m=>m.text||m),['head','page0','page1','tail0','tail1']);
  assert.equal(entry.partial.cursor,B);
  assert.equal(rendered.length,1);
  assert.match(gap.querySelector('.history-page-error').textContent,/specific 410/);
});

test('a delayed explicit window reload cannot overwrite an accepted live update', async () => {
  for (const mutate of [e=>{e.msgs=e.msgs.concat('live');},e=>{e.end=999;},
    e=>{e.version={head:'new'};},e=>{e.anchor='new';},e=>{e.activity={state:'aborted'};},
    e=>{e.meta={...e.meta,title:'updated'};},e=>{e.prompt={state:'waiting'};}]) {
    const {context,entry,gap}=fixture();let resolve,applied=0;
    const reload=element('button','history-gap-reload');gap.appendChild(reload);
    context.fetchMessages=()=>new Promise(done=>{resolve=done;});
    context.applyDiff=async()=>{applied++;};
    load(context,'reloadHistoryWindow');
    const task=context.reloadHistoryWindow('codex:fixture',null,reload);
    mutate(entry);
    resolve({data:{reset:true,meta:entry.meta,version:{head:'old'},messages:['stale']},bytes:1});
    await task;
    assert.equal(applied,0);assert.equal(entry.partial.cursor,A);
    assert.match(gap.querySelector('.history-page-error').textContent,/实时历史已更新/);
    assert.equal(context.historyPageRequests.size,0);
  }
});
