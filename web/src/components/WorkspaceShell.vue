<script setup lang="ts">
import SearchControls from './search/SearchControls.vue'
import SearchStatus from './search/SearchStatus.vue'
import SearchProgress from './search/SearchProgress.vue'
import SearchSummary from './search/SearchSummary.vue'
import { toggleResources } from '../services/shell/resources'
import HeaderActions from './shell/HeaderActions.vue'
import { state, operations } from '../stores/shell'
import { setSideCollapsed, startDrag, finishDrag, setSideWidth, SIDE_DEFAULT, sideResourceExtra } from '../services/shell/workspace'
defineProps<{ hostname: string }>()
</script>

<template>
  <header :class="{'header-fold-labels': state.labels, 'header-fold-brand': state.brand, 'header-fold-nodes': state.nodes}">
    <div class="brand">
      <span class="brand-name" :title="hostname">{{ hostname }}</span>
      <button class="btn" id="side-toggle" :title="state.collapsed ? '展开会话列表' : '收起会话列表'" :aria-label="state.collapsed ? '展开会话列表' : '收起会话列表'" :aria-expanded="state.collapsed ? 'false' : 'true'" @click="setSideCollapsed(!state.collapsed)">
        <svg class="ui-icon" aria-hidden="true"><use href="#i-sidebar"/></svg>
      </button>
    </div>
    <div class="header-filters">
      <div class="seg" id="session-scope" role="radiogroup" aria-label="会话范围">
        <button type="button" id="livecount" role="radio" aria-checked="false" tabindex="-1" title="只显示活跃会话"><b id="session-active">0</b></button>
        <button type="button" id="allcount" role="radio" aria-checked="true" tabindex="0" class="on" title="显示全部会话"><b id="session-total">0</b></button>
      </div>
      <div class="seg" id="sidebar-resource-control">
        <button type="button" id="sidebar-resources-toggle" :class="{on: state.resources}" :aria-pressed="state.resources ? 'true' : 'false'" aria-label="列表资源" :title="state.resources ? '隐藏列表资源列' : '显示列表资源列：CPU、进程、内存、GPU、磁盘读写'" @click.capture="toggleResources"><svg class="ui-icon" aria-hidden="true"><use href="#i-resource-cpu"/></svg><span class="mobile-label">资源</span></button>
      </div>
      <div class="seg" id="nest">
        <button type="button" id="nest-toggle" :class="{on: state.nest}" :aria-pressed="state.nest ? 'true' : 'false'" @click="operations().toggleNest()" title="分层显示：由会话发起的会话缩进在发起者之下；子代理始终挂在会话下面" aria-label="分层显示"><svg class="ui-icon" aria-hidden="true"><use href="#i-tree"/></svg><span class="mobile-label">分层</span></button>
      </div>
      <div class="seg" id="view" role="group" aria-label="列表视图">
        <button data-v="tree" :class="{on: state.view === 'tree'}" @click="operations().selectView('tree')" title="项目树" aria-label="项目树">📁 <span class="mobile-label">项目树</span></button>
        <button data-v="group" :class="{on: state.view === 'group'}" @click="operations().selectView('group')" title="会话分组" aria-label="会话分组">🏷 <span class="mobile-label">分组</span></button>
        <button data-v="date" :class="{on: state.view === 'date'}" @click="operations().selectView('date')" title="时间轴" aria-label="时间轴">🕒 <span class="mobile-label">时间轴</span></button>
      </div>
      <div class="node-picker" id="node-picker" hidden>
        <div class="seg" id="node-chips" role="group" aria-label="机器筛选"></div>
      </div>
      <div class="seg chips" id="chips" role="group" aria-label="Agent Type 筛选"></div>
    </div>
    <HeaderActions />
  </header>
  <div id="backend-notice" hidden role="status"></div>
  <div id="node-notice" hidden role="status"></div>
  <div id="prog"><div class="bar"></div><span class="txt"></span></div>
  <main>
    <div id="left" :style="{width: state.width}">
      <div class="side-search">
        <SearchControls />
        <div id="session-group-status" role="status" aria-live="polite"></div>
        <SearchStatus />
        <div class="side-tools" id="side-tools" hidden>
          <span id="side-picked"></span>
          <button type="button" class="btn" id="side-pick-all">全选</button>
          <button type="button" class="btn" id="side-pick-stop" disabled>停止</button>
          <button type="button" class="btn" id="side-pick-group" aria-controls="session-group-menu" aria-haspopup="menu" aria-expanded="false" disabled>分组</button>
          <button type="button" class="btn" id="side-pick-attach" disabled>附属到…</button>
          <button type="button" class="btn danger" id="side-pick-delete" disabled>删除</button>
          <button type="button" class="btn" id="side-pick-cancel">取消</button>
          <details id="side-stop-details" hidden>
            <summary id="side-stop-summary"></summary>
            <div id="side-stop-errors"></div>
          </details>
        </div>
        <SearchProgress />
      </div>
      <SearchSummary />
      <div id="side"><div class="spin">正在扫描会话…</div></div>
    </div>
    <div id="drag" title="拖动调整宽度，双击复位" :style="{transform: state.dragTransform}" @pointerdown="startDrag" @lostpointercapture="finishDrag" @dblclick="setSideWidth(SIDE_DEFAULT + sideResourceExtra(), true)"></div>
      <div id="right">
      <div id="detail"><div class="empty">从左侧选择一个会话</div></div>
      <div id="composer-root"></div>
      <div id="termpane" class="hidden">
        <div class="term-resizer" id="tgrip" title="拖动终端上边界调整高度"></div>
        <span id="term-ctrl-lock" role="status">Ctrl（下一键）</span>
        <div id="xterm"></div>
        <div id="term-output-notice" role="status" hidden></div>
        <div id="term-timeline" class="term-timeline" aria-label="录制回放进度">
          <span id="tl-status" role="status">会话已结束 · 只读回放</span>
          <button type="button" id="tl-play" title="播放" aria-label="播放">▶</button>
          <div class="tl-track">
            <input type="range" id="tl-seek" min="0" max="1000" value="1000" step="1" aria-label="回放进度">
            <div id="tl-ticks" aria-hidden="true"></div>
          </div>
          <span id="tl-time">00:00 / 00:00</span>
          <select id="tl-speed" aria-label="倍速"><option value="1">1×</option><option value="2">2×</option><option value="4">4×</option><option value="8">8×</option><option value="16">16×</option></select>
          <button type="button" id="tl-live" title="跳到最新并跟随" aria-label="跳到最新并跟随" hidden>最新</button>
        </div>
        <div class="term-keys" aria-label="终端快捷键">
          <button data-term-modifier="ctrl" title="Ctrl（下一键生效）" aria-label="Ctrl，下一键生效" aria-pressed="false">Ctrl</button>
          <button data-term-modifier="alt" title="Alt（下一键生效）" aria-label="Alt，下一键生效" aria-pressed="false">Alt</button>
          <button data-term-modifier="shift" title="Shift 选字：开启后拖动选择文字，绕过 CLI 鼠标捕获；再次点击关闭" aria-label="Shift，锁定本地选字" aria-pressed="false">Shift</button>
          <button data-term-key="Escape" title="Escape" aria-label="Escape">Esc</button>
          <button data-term-key="Tab" title="Tab" aria-label="Tab">Tab</button>
          <button data-term-key="Left" title="方向键左" aria-label="方向键左">←</button>
          <button data-term-key="Up" title="方向键上" aria-label="方向键上">↑</button>
          <button data-term-key="Down" title="方向键下" aria-label="方向键下">↓</button>
          <button data-term-key="Right" title="方向键右" aria-label="方向键右">→</button>
          <button data-term-key="PPage" title="Page Up" aria-label="Page Up">Pg↑</button>
          <button data-term-key="NPage" title="Page Down" aria-label="Page Down">Pg↓</button>
        </div>
      </div>
    </div>
  </main>
</template>
