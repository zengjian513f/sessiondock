'use strict';
// Conversation references are resolved here; browsing and preview belong to FileDock.
(async () => {
  const base=new URL('.',location.href), context=new URLSearchParams(location.search);
  const host=document.getElementById('file-content');
  try {
    let node=context.get('node') || (context.get('uid') || '').match(/^[^:]+:([a-f0-9]{32})~/)?.[1];
    if(!node){
      const response=await fetch(new URL('api/meta',base));
      if(!response.ok) throw new Error('无法读取机器信息');
      node=(await response.json()).node_id;
    }
    if(!node) throw new Error('未找到文件所在机器');
    let path=context.get('path');
    if(!path){
      const ref=context.get('ref');
      if(!ref) throw new Error('缺少文件路径');
      const response=await fetch(new URL('api/session/resolve-files',base),{
        method:'POST',headers:{'Content-Type':'application/json'},
        body:JSON.stringify({uid:context.get('uid'),agent:context.get('agent') || '',refs:[ref]}),
      });
      if(!(response.headers.get('Content-Type') || '').includes('application/json')) throw new Error('请登录后刷新页面');
      const result=await response.json();
      if(!response.ok) throw new Error(result.error || '无法解析文件路径');
      path=result.targets?.find(item=>item.ref===ref)?.path || result.resolved?.[ref];
      if(!path) throw new Error(result.errors?.find(item=>item.ref===ref)?.error || '无法确定文件的完整路径');
    }
    const service=new URL(SessionDockCapabilities.config.filedock_url || '/files/',location.href);
    if(!service.pathname.endsWith('/')) service.pathname+='/';
    const destination=new URL('file.html',service);
    destination.search=new URLSearchParams({node,path});destination.hash=location.hash;
    location.replace(destination);
  } catch(error){
    document.title='无法打开文件 · SessionDock';
    document.getElementById('file-title').textContent='无法打开文件';
    document.querySelector('header').hidden=false;
    const retry=document.getElementById('file-retry');retry.hidden=false;retry.onclick=()=>location.reload();
    host.textContent=error.message || '无法打开文件';
  }
})();
