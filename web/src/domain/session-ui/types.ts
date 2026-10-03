export interface NodeOption {id:string;label:string;disabled:boolean}
export interface SourceOption {value:string;disabled:boolean;title:string}
export interface ModelRow {id:string;name?:string;efforts?:string[];default_effort?:string}
export interface ModelPresentation {
 rows:ModelRow[];efforts:string[];model:string;effort:string;label:string;title:string;
 enabled:boolean;effortTitle:string;open:boolean;searchVisible:boolean;query:string;active:number
}
export interface CwdRow {section?:string;note?:string;path?:string;meta?:string;kind?:string;index?:number}
export interface SessionRow {
 uid:string;sid:string;title:string;cwd?:string;source:string;created?:string;updated?:string;
 agent_id?:string;fork_parent?:boolean;fork_parent_visible?:boolean;forked_from_id?:string
}
export interface HeaderAction {
 id?:string;kind?:string;class?:string;label?:string;icon?:string;iconHtml?:string;reason?:string;
 pressed?:boolean|'true'|'false';starUid?:string;disabled?:boolean;hidden?:boolean;available?:boolean;order?:number;
 run?:((button:HTMLButtonElement|null,onPending?:(pending:boolean)=>void)=>unknown)|null
}
export interface HeaderMetadata {id?:string;class?:string;color?:string;code?:boolean;text:string}
export interface SessionView {id:string;on:boolean;running:boolean;main:boolean;type?:string;span?:string;title:string}
export interface HeaderPresentation {
 meta:SessionRow;pending:unknown;title:string;total:number;hasAgents:boolean;icon:string;menuOpen:boolean;
 viewsOpen:boolean;chainOpen:boolean;views:SessionView[];chain:{sid?:string;row?:SessionRow;level:string}[];
 inline:number;brief:number;moreHidden:boolean;moreLabel:string;items:HeaderAction[];metadata:HeaderMetadata[];
 freeze:HeaderAction;action:HeaderAction;chainPending:string[]
}
export interface TransferMember {source:string;sid:string;selected:boolean;name:string;detail:string;detailTitle:string;sourceName:string;badge:string;files:number;sizeLabel:string}
export interface TransferTask {request:{operation_id:string;uid:string;target_node:string};plan?:unknown}
export interface TransferPresentation {
 members:TransferMember[];options:{value:string;label:string;disabled:boolean}[];sourceName:string;
 targetNode:string;mode:'clone'|'move';newIds:boolean;identityVisible:boolean;notice:string;
 confirmLabel:string;confirmDisabled:boolean;controlsDisabled:boolean;abortVisible:boolean;abortLabel:string;
 aborting:boolean;busy:boolean;status:string;environmentVisible:boolean;environmentText:string;environmentTitle:string;
 progressVisible:boolean;progressText:string;progressPhase:string;errorVisible:boolean;errorText:string;
 actions:{targetChanged:(event:Event)=>void;modeChanged:(event:Event)=>void;identityChanged:(event:Event)=>void;
 close:()=>unknown;cancel:(event:Event)=>void;abort:()=>unknown;confirm:()=>unknown}
}
export interface TransferTasksPresentation {rows:{id:string;title:string;nodes:string;phase:string;task:TransferTask}[];pending:string[];errorText:string}
export interface TrashItem {id:string;title:string;icon:string;when:string;sizeLabel:string;cwd:string;directory:string;restorable:boolean;origin:string;originLabel:string;reason?:string}
export interface Attachment {
 id:string;kind:string;number:number;file?:File;uploaded?:{name?:string;size?:number};preview?:string;
 status?:string;progress?:number;error?:string;cancelUpload?:()=>void
}
export interface AttachmentPresentation extends Attachment {raw:Attachment}
export interface BugToast {report:string;worker:{name:string;source?:string;node_name?:string};text:string}
export interface SessionUiPresentation {
 header:HeaderPresentation|null;consoleToast:string;frozen:boolean;freezeUid:string;stop:string;stopUid:string;stopHidden:boolean;
 trash:TrashItem[];trashEmpty:string;trashSub:string;trashNote:string;trashError:boolean;trashDisabled:boolean;trashPending:string;
 transfer:TransferPresentation|null;tasks:TransferTasksPresentation|null;
 bugAttachments:AttachmentPresentation[];bugStorageError:string;bugToast:BugToast|null;
 bugNodes:NodeOption[];bugNode:string;bugNodeVisible:boolean;bugNodeDisabled:boolean;bugSources:SourceOption[];bugSource:string;
 bugText:string;bugError:string;bugInputDisabled:boolean;bugAddDisabled:boolean;bugSubmitDisabled:boolean;
 bugSubmitLabel:string;bugBusyLabel:string;bugAttachOpen:boolean;bugAttachPadding:string;bugSending:boolean;
 newNodes:NodeOption[];newNode:string;newNodeVisible:boolean;newSources:SourceOption[];newSource:string;newCwd:string;
 newError:string;newSubmitDisabled:boolean;newSubmitLabel:string;cwdRows:CwdRow[];cwdVisible:boolean;cwdActive:number;cwdTitle:string;cwdStatus:string;
 models:Record<string,ModelPresentation>
}

export interface ModelController {
 model:string;effort:string;refresh:()=>Promise<void>;choice:()=>{model?:string;effort?:string};
 close:(focus?:boolean)=>void;outside:(event:Event)=>void;toggle:()=>unknown;buttonKey:(event:KeyboardEvent)=>void;
 search:(event:Event)=>void;keydown:(event:KeyboardEvent)=>void;choose:(index:number)=>void;effortChanged:(event:Event)=>void
}
export interface AppController {
 forkLabel:string;formatTime:(value?:string)=>string;headerMounted:()=>unknown;consoleClick:(button:EventTarget|null)=>unknown;
 chainVisibility:(row:SessionRow,button?:EventTarget|null)=>unknown;chainOpen:(row:SessionRow)=>unknown;
 mobileBack:()=>unknown;closeSessionActions:(focus?:boolean)=>void;toggleViews:(event:Event)=>void;
 selectView:(id:string,event:Event)=>void;toggleChain:(event:Event)=>void;toggleActions:()=>void;
 actionKey:(event:KeyboardEvent)=>void;menuKey:(event:KeyboardEvent)=>void;focusout:(event:FocusEvent)=>void;
 toggleStar:()=>unknown;toggleTurns:()=>unknown;cloneGroup:()=>unknown;freezeSession:(button:EventTarget|null)=>unknown;
 sessionAction:(button:EventTarget|null)=>unknown;newSessionProxy:()=>unknown;settingsProxy:()=>unknown;
 reloadProxy:()=>unknown;transfersProxy:()=>unknown;consoleHint:()=>unknown;clearConsoleHint:()=>void;
 openTrash:()=>void;trashItemAction:(event:MouseEvent)=>unknown;loadTrash:()=>unknown;purgeAllTrash:()=>unknown;
 closeTransferTasks:()=>void;continueTransferTask:(task:TransferTask)=>unknown;
 sessionViewRows:(meta:SessionRow)=>SessionView[];setActionPending:(pending:boolean)=>void
}
export interface LaunchController {
 NewModels:ModelController;BugReportModels:ModelController;bugReportSending:boolean;
 prepareNewNode:()=>Promise<void>;refreshNewNodeFields:(keepCwd?:string)=>void;newNodeId:()=>string;newDirsKey:()=>string;
 newPointerdown:(event:PointerEvent)=>void;newPointercancel:()=>void;newBackdropClick:(event:MouseEvent)=>void;
 newClosed:()=>void;createNewSession:(event:SubmitEvent)=>unknown;newNodeChanged:(event:Event)=>void;newSourceChanged:(event:Event)=>void;
 cwdInput:(event:Event)=>void;cwdKeydown:(event:KeyboardEvent)=>void;cwdChoose:(event:MouseEvent)=>void;
 bugPointerdown:(event:PointerEvent)=>void;bugPointercancel:()=>void;bugClick:(event:MouseEvent)=>void;bugClosed:()=>void;
 submitBugReport:(event:SubmitEvent)=>unknown;bugPaste:(event:ClipboardEvent)=>void;bugNodeChanged:(event:Event)=>void;
 bugSourceChanged:(event:Event)=>void;bugAttachToggle:(event:MouseEvent)=>unknown;bugChooseFile:(kind:string)=>void;
 bugFileChanged:(event:Event)=>void;bugInput:(event:Event)=>void;bugKeydown:(event:KeyboardEvent)=>void;
 bugInsert:(number:number)=>void;bugRetry:(attachment:Attachment)=>void;bugRemove:(attachment:Attachment)=>void;
 formatSize:(size:number)=>string;kindIcon:(kind:string)=>string;dismissBugToast:()=>void;openBugToast:()=>unknown
}
