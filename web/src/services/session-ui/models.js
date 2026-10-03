

import {reactive,markRaw,nextTick} from 'vue'
/** @param {import('../../domain/session-ui/types').SessionUiPresentation} ui */
export function createModelService(bridge, ui) {
const MODEL_SEARCH_MIN = 10;
const modelCatalogs = new Map();

function fetchModelCatalog(node, source) {
  const key = (bridge.environment.HUB_MODE ? node + '|' : '') + source;
  if (!modelCatalogs.has(key)) {
    modelCatalogs.set(key, (async () => {
      try {
        const params = new URLSearchParams({source, ...(bridge.environment.HUB_MODE ? {node} : {})});
        const response = await bridge.runtime().network.fetch(bridge.environment.appUrl(`api/term/models?${params}`), {cache: 'no-store'});
        const data = response.ok ? await response.json() : null;
        return Array.isArray(data?.models) ? data : null;
      } catch { return null; }
    })().then(catalog => {
      if (!catalog) modelCatalogs.delete(key); // 下次打开再试
      return catalog;
    }));
  }
  return modelCatalogs.get(key);
}

/** 模型按机器和来源记住；强度按来源和模型共享，不区分机器或弹窗。 */
function createModelPicker(prefix, {source, node, storeKey}) {
  const el = name => document.getElementById(`${prefix}-${name}`);
  const button = el('model'), menu = el('model-menu');
  const search = el('model-search'), box = el('model-options');
  const view = reactive({rows:[], efforts:[], model:'', effort:'', label:'读取模型…', title:'模型', enabled:false, effortTitle:'推理强度', open:false, searchVisible:false, query:'', active:-1}); ui.models = {...ui.models, [prefix]:view};
  const picker = {catalog: null, key: '', model: '', effort: '', rows: [], active: -1, seq: 0};
  const info = id => picker.catalog?.models.find(model => model.id === id) || null;
  const controls = (text, title, enabled) => {
    Object.assign(view,{label:text,title,enabled});
  };
  picker.apply = (model, effort, persist = false) => {
    const catalog = picker.catalog, chosen = info(model);
    const shown = chosen || info(catalog?.default_model);
    picker.model = shown?.id || '';
    const efforts = shown?.efforts || catalog?.efforts || [];
    const effortKey = `modelEffort.${source()}|${picker.model}`;
    const remembered = bridge.preferences.get(effortKey, '');
    picker.effort = efforts.includes(effort) ? effort : efforts.includes(remembered) ? remembered
      : efforts.includes('high') ? 'high'
      : efforts.includes(shown?.default_effort) ? shown.default_effort : efforts[0] || '';
    const name = shown ? shown.name || shown.id : catalog?.models.length ? '选择模型' : '模型不可用';
    controls(name, shown && shown.name !== shown.id ? `${shown.name}（${shown.id}）` : '模型：' + name,
      !!catalog?.models.length);
    view.model=picker.model; view.effort=picker.effort; view.efforts=[...efforts];
    view.effortTitle = efforts.length ? '推理强度' : '该 CLI 不支持选择推理强度';
    if (persist) bridge.preferences.set(`${storeKey}.${picker.key}`, {model: picker.model});
    if (persist && effort && picker.model && efforts.includes(effort)) bridge.preferences.set(effortKey, effort);
  };
  picker.refresh = async () => {
    const current = source(), where = node(), seq = ++picker.seq;
    picker.close();
    picker.key = (bridge.environment.HUB_MODE ? where + '|' : '') + current;
    picker.catalog = null;
    picker.model = picker.effort = '';
    picker.apply('', '');
    if (!current || current === 'shell') {
      controls('模型不可用', '终端会话不选择模型', false);
      return;
    }
    controls('读取模型…', '正在读取该 CLI 的模型列表', false);
    const catalog = await fetchModelCatalog(where, current);
    if (seq !== picker.seq) return;
    picker.catalog = catalog && markRaw(catalog);
    const saved = bridge.preferences.get(`${storeKey}.${picker.key}`, {}) || {};
    picker.apply(saved.model || '', '');
    if (!catalog) controls('模型不可用', '该机器没有返回模型列表，将使用 CLI 默认模型', false);
  };
  /** 请求体里的具体选择；目录不可用时才由 CLI 自行决定。 */
  picker.choice = () => ({...(picker.model ? {model: picker.model} : {}),
    ...(picker.effort ? {effort: picker.effort} : {})});
  picker.close = (focus = false) => {
    if (!view.open) return;
    if (menu.matches(':popover-open')) menu.hidePopover();
    view.open = false;
    if (focus) button.focus();
  };
  const setActive = (index, scroll = true) => {
    const options = [...box.querySelectorAll('[data-model-option]')];
    picker.active = options.length ? (index + options.length) % options.length : -1;
    view.active = picker.active;
    const active = options[picker.active];
    if (active && scroll) active.scrollIntoView({block: 'nearest'});
  };
  const render = () => {
    const query = view.query.trim().toLocaleLowerCase();
    const all = picker.catalog.models;
    picker.rows = query ? all.filter(model => model.id
      && `${model.id} ${model.name || ''}`.toLocaleLowerCase().includes(query)) : all;
    view.rows = picker.rows.map(model=>({id:model.id,name:model.name}));
    const selected = picker.rows.findIndex(model => model.id === picker.model);
    nextTick(() => setActive(query ? 0 : Math.max(0, selected), !query));
  };
  const open = async () => {
    if (!picker.catalog?.models.length) return;
    view.searchVisible = picker.catalog.models.length > MODEL_SEARCH_MIN;
    view.query = '';
    view.open = true;
    await nextTick();
    menu.showPopover();
    render();
    await nextTick();
    place();
    (!view.searchVisible ? box : search).focus();
  };
  // 浮层与模型+强度这一组等宽，下方放不下时翻到上方。getBoundingClientRect 是缩放后的
  // 像素，写回 style 前除以界面缩放。
  const place = () => {
    const zoom = menu.currentCSSZoom || 1, gap = 4, edge = 8;
    const group = button.closest('.new-choice').getBoundingClientRect();
    const pick = button.getBoundingClientRect();
    const below = innerHeight - pick.bottom - gap - edge, above = pick.top - gap - edge;
    const up = below < 160 && above > below;
    menu.style.left = `${group.left / zoom}px`;
    menu.style.width = `${group.width / zoom}px`;
    menu.style.maxHeight = `${Math.max(0, up ? above : below) / zoom}px`;
    const top = up ? pick.top - gap - menu.getBoundingClientRect().height : pick.bottom + gap;
    menu.style.top = `${top / zoom}px`;
  };
  addEventListener('resize', () => { if (view.open) place(); });
  addEventListener('scroll', e => { if (view.open && !menu.contains(e.target)) place(); }, true);
  const choose = index => {
    const model = picker.rows[index];
    if (!model) return;
    picker.apply(model.id, '', true);
    picker.close(true);
  };
  picker.toggle = () => (!view.open ? open() : picker.close());
  picker.buttonKey = e => {if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {e.preventDefault(); open();}};
  picker.search = event => {view.query=event.target.value;render();};
  picker.keydown = e => {
    if (e.isComposing) return;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {e.preventDefault(); setActive(picker.active + (e.key === 'ArrowDown' ? 1 : -1));}
    else if (e.key === 'Enter') {e.preventDefault(); choose(picker.active);}
    else if (e.key === 'Escape') {e.preventDefault(); e.stopPropagation(); picker.close(true);}
    else if (e.key === 'Tab') picker.close();
  };
  picker.choose = choose;
  picker.effortChanged = e => picker.apply(picker.model, e.currentTarget.value, true);
  picker.outside = e => {if (!button.parentElement.contains(e.target)) picker.close();};
  return picker;
}

return {createModelPicker, modelCatalogs, fetchModelCatalog};
}
