import * as Sidebar from '../../migration/sidebar'
import * as Shell from '../../migration/shell'
import {state as shellState} from '../../stores/shell'
export function createSidebarResources(core,terminal,sessionUi){
 let enabled=!!core.preferences.get('sidebarResources',false), rows=new Map(),localNode='',sampledAt=0,pending=false;
 const key=s=>JSON.stringify([s.node_id||localNode,s.source,s.sid]);
 const cells=(session,agent)=>Sidebar.Resources.cells(session,agent,rows,localNode,sampledAt);
 const paintVisible=()=>Sidebar.refreshResources(enabled);
 async function refresh(){if(!enabled||pending||document.hidden)return;pending=true;try{const response=await core.network.fetch(core.environment.appUrl('api/resources/summary'),{signal:AbortSignal.timeout(4500)});if(!response.ok)throw new Error('resource summary unavailable');const data=await response.json();if(!Array.isArray(data.sessions)||!Number.isFinite(data.sampled_at))throw new Error('invalid summary');localNode=data.node_id||'';sampledAt=data.sampled_at;rows=new Map(data.sessions.map(row=>[key(row.session),row.metrics]));}catch{rows=new Map();sampledAt=0}finally{pending=false;paintVisible()}}
 function toggle(value=!enabled,save=true){enabled=value;if(save)core.preferences.set('sidebarResources',enabled);document.body.classList.toggle('sidebar-resources',enabled);shellState.resources=enabled;Shell.setSideWidth(core.preferences.get('width',Shell.SIDE_DEFAULT)+Shell.sideResourceExtra());paintVisible();sessionUi().layoutSessionHead();requestAnimationFrame(()=>{if(terminal().state.term)terminal().fitTerm()});if(enabled)void refresh()}
 function start(){document.addEventListener('visibilitychange',()=>{if(!document.hidden){paintVisible();void refresh()}});setInterval(()=>{if(!document.hidden){paintVisible();void refresh()}},5000);toggle(enabled,false)}
 return {cells,refresh,toggle,start}
}
