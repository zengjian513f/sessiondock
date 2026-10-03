<script setup lang="ts">
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
        <div class="qbox">
          <input id="q" autocomplete="off" aria-label="搜索会话" placeholder="快速筛选… Enter 搜正文" title="无需回车：筛选标题、UUID / UID、目录、机器名、Agent 名和模型名；空格分词，双引号搜索短语；Enter 搜索对话正文">
          <div class="opts" id="search-mode" role="group" aria-label="关键词匹配方式">
            <button type="button" id="search-mode-toggle" class="on" title="全部词（AND），点击切换为任一词（OR）" aria-label="全部词（AND），点击切换为任一词（OR）">AND</button>
          </div>
          <div class="opts" id="opts">
            <button type="button" data-o="case" title="大小写敏感" aria-label="大小写敏感" aria-pressed="false">Aa</button>
            <button type="button" data-o="word" title="全词匹配" aria-label="全词匹配" aria-pressed="false">ab|</button>
            <button type="button" data-o="regex" title="正则表达式：整段输入作为正则，不拆词" aria-label="正则表达式" aria-pressed="false">.*</button>
          </div>
        </div>
        <div id="session-group-status" role="status" aria-live="polite"></div>
        <div id="stat" role="status">加载中…</div>
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
        <div id="search-progress" role="status" aria-live="polite">
          <div class="search-progress-head"><span>全文搜索</span><b>准备中…</b><button type="button" class="btn" id="search-cancel">取消</button></div>
          <div class="search-progress-track" role="progressbar" aria-label="会话扫描进度" aria-valuemin="0" aria-valuemax="100"><i></i></div>
          <div class="search-progress-nodes"></div>
        </div>
      </div>
      <div id="side-search-state" hidden>
        <div class="side-search-summary"><strong>⌕ <span id="side-search-label">搜索结果</span></strong><span id="side-search-query"></span><span id="side-search-count"></span></div>
        <button type="button" class="btn" id="side-search-exit">退出搜索 ×</button>
      </div>
      <div id="side"><div class="spin">正在扫描会话…</div></div>
    </div>
    <div id="drag" title="拖动调整宽度，双击复位" :style="{transform: state.dragTransform}" @pointerdown="startDrag" @lostpointercapture="finishDrag" @dblclick="setSideWidth(SIDE_DEFAULT + sideResourceExtra(), true)"></div>
      <div id="right">
      <div id="detail"><div class="empty">从左侧选择一个会话</div></div>
      <div id="composer" class="hidden">
        <div id="composer-question" class="hidden" aria-label="CLI 选择题"></div>
        <div id="composer-input-status" class="hidden" role="status" aria-live="polite" aria-atomic="true"></div>
        <div id="compose-items" aria-live="polite"></div>
        <div class="composer-row">
          <div class="attach-picker">
            <button class="btn" id="cadd" type="button" title="添加附件或引用" aria-label="添加附件或引用" aria-expanded="false">
              <svg class="ui-icon" aria-hidden="true"><use href="#i-plus"/></svg>
            </button>
            <div class="attach-menu hidden" id="attach-menu" role="menu">
              <button type="button" data-attach="image" role="menuitem"><span>▧</span>图片</button>
              <button type="button" data-attach="video" role="menuitem"><span>▶</span>视频</button>
              <button type="button" data-attach="audio" role="menuitem"><span>♪</span>音频</button>
              <button type="button" data-attach="file" role="menuitem"><span>⌑</span>文件</button>
              <button type="button" data-attach="quote" role="menuitem"><span>❝</span>引用文字</button>
            </div>
            <input id="cfile" type="file" multiple hidden>
          </div>
          <div class="composer-input-wrap">
            <div id="input-history" class="input-history hidden" role="listbox"
              aria-label="输入历史"></div>
            <textarea id="cinput" rows="1" enterkeyhint="enter" placeholder="输入内容"
              aria-controls="input-history" aria-expanded="false"></textarea>
          </div>
          <div class="cbtns">
            <button class="btn" id="cesc"
              title="单击向 CLI 发送 Esc；Claude 中双击进入原生回滚选择"
              aria-label="单击发送 Esc；Claude 中双击进入回滚选择">Esc</button>
            <button class="btn go" id="csend">发送</button>
          </div>
        </div>
      </div>
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
