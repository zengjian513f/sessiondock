export function attachmentIcon(kind:string) {
  return ({image:'▧',video:'▶',audio:'♪',file:'⌑'} as Record<string,string>)[kind] || '⌑'
}
export function attachmentSummary(attachment:any, size:(value:number)=>string) {
  const kind=attachment.kind==='file' ? '文件' : ({image:'图片',video:'视频',audio:'音频'} as Record<string,string>)[attachment.kind]
  const summary=`[附件${attachment.number}] · ${kind} · ${size(attachment.file?.size ?? attachment.uploaded?.size ?? 0)}`
  return attachment.status==='uploading' ? `${summary} · ${attachment.progress || 0}%`
    : attachment.status==='queued' ? `${summary} · 等待上传`
      : attachment.status==='failed' ? `${summary} · ${attachment.error || '上传失败'}` : summary
}
