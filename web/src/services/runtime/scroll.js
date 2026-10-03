import {useConversationScrollStore} from '../../stores/runtime/scroll'

// Message viewport anchoring and observer lifetime are scoped operational state.
export function createConversationScroll(pinia, flushPendingTurnSeal) {
  const $ = selector => document.querySelector(selector);
const BOTTOM_SLACK = 48;
const state = useConversationScrollStore(pinia);

/** 窗口缩放会让浏览器自己调整滚动位置, 那一小段时间内的滚动不算用户意图。
 *  只在 window resize 时用 —— 别在内容变化时也锁, 否则锁会被不断续期,
 *  用户主动往上翻都会被忽略。 */
function lockStick(ms = 300) { state.lockUntil = performance.now() + ms; }

function atBottom(box) {
  return box.scrollHeight - box.scrollTop - box.clientHeight <= BOTTOM_SLACK;
}

function stickBottom(box, force) {
  if (force) state.stick = true;
  if (!state.stick) return;
  state.selfScroll = true;
  box.scrollTop = box.scrollHeight;
  // 必须读回来: 浏览器会把它夹到 maxScroll, 直接记 scrollHeight 的话
  // 下一次 scroll 事件会把这当成"用户往上翻了一大截", 跟随就断了
  state.lastTop = box.scrollTop;
  requestAnimationFrame(() => { state.selfScroll = false; });
}

/** 展开/收起大块内容时把触发控件钉在原来的视口坐标。用户点开过程是在看
 *  这一段，不再属于“持续跟随最底部”；否则 ResizeObserver 会把按钮直接
 *  推出屏幕。补两帧可覆盖语法高亮等紧随其后的同步布局变化。 */
function mutateKeepingMessageAnchor(anchor, mutate) {
  const box = $('#msgs');
  if (!box || !box.contains(anchor)) return mutate();
  const top = anchor.getBoundingClientRect().top;
  state.stick = false;
  state.lastTop = box.scrollTop;
  const restore = () => {
    if (!anchor.isConnected || box !== $('#msgs')) return;
    const delta = anchor.getBoundingClientRect().top - top;
    if (Math.abs(delta) < .5) return;
    state.selfScroll = true;
    box.scrollTop += delta;
    state.lastTop = box.scrollTop;
    requestAnimationFrame(() => { state.selfScroll = false; });
  };
  const result = mutate();
  restore();
  requestAnimationFrame(() => {
    restore();
    requestAnimationFrame(restore);
  });
  return result;
}

function jumpWithinConversation(target, block = 'center') {
  const box = $('#msgs');
  if (!target || !box?.contains(target)) return;
  state.stick = false;
  state.lastTop = box.scrollTop;
  target.scrollIntoView({block, behavior: 'smooth'});
}

function disposeMessageObservers(box) {
  box?._ro?.disconnect();
  box?._mo?.disconnect();
}

function watchBottom(box) {
  state.stick = true;
  state.lastTop = box.scrollTop;
  // 按滚动"方向"判断意图, 而不是按当前是否贴底 —— 窗口缩小、内容展开都会让
  // "是否贴底"瞬间变假, 那不是用户想离开底部。
  // 直接听用户的动作来判断"想离开底部"。不能只靠 scroll 事件推方向:
  // 布局重排、程序自身的滚动都会产生 scroll, 混在一起分不清谁是谁。
  const leave = () => { state.stick = false; };
  box.addEventListener('wheel', e => { if (e.deltaY < 0) leave(); }, { passive: true });
  box.addEventListener('touchmove', leave, { passive: true });
  box.addEventListener('keydown', e => {
    if (['PageUp', 'ArrowUp', 'Home'].includes(e.key)) leave();
  });
  box.addEventListener('scroll', () => {
    const top = box.scrollTop;
    // 拖滚动条没有 wheel 事件, 靠方向补一手 (排除程序自己滚的那些)
    if (!state.selfScroll && performance.now() >= state.lockUntil && top < state.lastTop - 2) state.stick = false;
    else if (atBottom(box)) {
      state.stick = true;                               // 回到底部就恢复跟随
      if (box._turnSealPending) queueMicrotask(() => flushPendingTurnSeal(box));
    }
    state.lastTop = top;
  });
  // 内容高度变化 (展开消息、渲染完成、字体加载…) 时跟随
  if (window.ResizeObserver) {
    disposeMessageObservers(box);
    const ro = new ResizeObserver(() => settle(box));
    ro.observe(box);
    // 普通消息只盯底部 30 条，避免上万节点的观察成本；但图片、公式、表格即使
    // 位于很早的消息里，加载或窄屏重排也会改变总高度，必须额外观察。
    const rich = new Set(), ordinary = new Set();
    const observeRich = root => {
      const observe = node => { rich.add(node); ro.observe(node); };
      if (root.matches?.('img, .katex, .tw')) observe(root);
      root.querySelectorAll?.('img, .katex, .tw').forEach(observe);
    };
    const syncTail = () => {
      const tail = new Set();
      for (let node = box.lastElementChild; node && tail.size < 30; node = node.previousElementSibling) tail.add(node);
      for (const node of ordinary) if (!tail.has(node)) { ordinary.delete(node); if (!rich.has(node)) ro.unobserve(node); }
      for (const node of tail) if (!ordinary.has(node)) { ordinary.add(node); ro.observe(node); }
      for (const node of rich) if (!box.contains(node)) { rich.delete(node); ro.unobserve(node); }
    };
    syncTail();
    observeRich(box);
    box._ro = ro;
    const mo = new MutationObserver(ms => {
      for (const m of ms) for (const n of m.addedNodes) {
        if (n.nodeType === 1) observeRich(n);
      }
      syncTail();
      settle(box);
    });
    mo.observe(box, { childList: true, subtree: true });
    box._mo = mo;
  }
}

/** 布局要好几帧才稳: 窗口变窄会让消息重新折行, scrollHeight 一路涨,
 *  只修一次会差一截。补几帧, 直到不再变化。 */
let _settleTick = 0;

function settle(box) {
  if (!state.stick) return;
  stickBottom(box);
  if (_settleTick) return;                   // 已经有补偿在跑, 别叠加
  let n = 0, last = -1;
  const tick = () => {
    _settleTick = 0;
    // 布局稳定(高度不再变)就收手 —— 一直空转的话 state.selfScroll 长期为真,
    // 用户主动往上翻会被当成程序自己滚的而忽略掉
    if (!state.stick || n++ > 10 || box !== $('#msgs') || box.scrollHeight === last) return;
    last = box.scrollHeight;
    stickBottom(box);
    _settleTick = requestAnimationFrame(tick);
  };
  _settleTick = requestAnimationFrame(tick);
}

const resize = () => {
  const box = $('#msgs');
  if (!box) return;
  lockStick();
  settle(box);
  setTimeout(() => settle(box), 160);      // 兜住 resize 之后的异步重排
  setTimeout(() => settle(box), 400);
};
  function start() {
    addEventListener('resize', resize);
    return () => removeEventListener('resize', resize);
  }
  return {state, start, lockStick, atBottom, stickBottom, mutateKeepingMessageAnchor, jumpWithinConversation, disposeMessageObservers, watchBottom, settle};
}
