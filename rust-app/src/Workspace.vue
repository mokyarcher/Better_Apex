<script setup lang="ts">
import {ref,computed,onMounted,onBeforeUnmount,watch} from 'vue';
import {invoke} from '@tauri-apps/api/core';
import {listen} from '@tauri-apps/api/event';
import {open,save} from '@tauri-apps/plugin-dialog';
import Icon from './Icon.vue';
import {createRefreshQueue,historySelection} from './historySync';
type Page='home'|'history'|'import'|'settings';
type NicknameDebugStatus={enabled:boolean;directory:string;current_file:string|null;error:string};
type OcrBackend='windows_tesseract'|'pp_ocr_v6_small';
type OcrSettings={backend:OcrBackend;ppocr_ready:boolean;error:string};
const ocrLabels:Record<OcrBackend,string>={windows_tesseract:'Windows OCR + Tesseract',pp_ocr_v6_small:'PP-OCRv6 small'};
type Target={handle:number;pid:number;title:string};
type Capture={running:boolean;status:string;frames:number;attempts:number;width:number;height:number;interval_ms:number;preview?:string|null};
type AutomationStage='waiting'|'loading'|'legend_select'|'in_match'|'settlement'|'saving'|'saved';
type LocalStoreStatus={data_dir:string;database_path:string;record_count:number;schema_version:number};type AutomationStatus={running:boolean;stage:AutomationStage;phase:string;pause_reason:string;roster:string;last_saved:string;saved_count:number;processed_frames:number;interval_ms:number;error:string};
type Player={slot:number;nickname:string|null;name_confirmed:boolean;is_local:boolean|null;kills:number|null;assists:number|null;knocks:number|null;damage:number|null};
type Match={id:string;saved_at:string;rank:number|null;squad_kills:number|null;players:Player[];has_settlement_image?:boolean;has_selection_image?:boolean};
type SaveMatchResult={id:string;saved_at:string;deduplicated:boolean};
type PlayerAnnotation={nickname:string|null;nickname_source:'manual';name_confirmed:boolean;is_local:boolean|null};
type Preview={image:string;width:number;height:number;name:string};
type NumberField={raw_text:string;value:number|null;confidence:number};
type SettlementPlayer={position:string;nickname_hint:string|null;name_confirmed:boolean;kills:NumberField;assists:NumberField;knocks:NumberField;damage:NumberField};
type SettlementResult={rank:NumberField;squad_kills:NumberField;players:SettlementPlayer[];warnings:string[]};
type SelectionEvidence={position:string;nickname:string;upper_left:string;lower_bar:string};
type RosterDraft={path:string;members:{position:string;nickname:string}[]};
type Probe={nickname_backend:OcrBackend;windows_text:string[];tesseract_text:string[];scope:string;settlement:SettlementResult|null;selection:SelectionEvidence[]|null};
const page=ref<Page>('home'),busy=ref(''),error=ref(''),notice=ref('');
const activeId=ref(''),demo=ref(false);
const nicknameDebug=ref<NicknameDebugStatus|null>(null);
const ocrSettings=ref<OcrSettings|null>(null),ocrChoice=ref<OcrBackend>('pp_ocr_v6_small');
async function refreshOcrSettings(){const next=await invoke<OcrSettings>('ocr_settings');ocrSettings.value=next;ocrChoice.value=next.backend;}
async function saveOcrBackend(){await run('ocr-settings',async()=>{const next=await invoke<OcrSettings>('set_ocr_backend',{backend:ocrChoice.value});ocrSettings.value=next;ocrChoice.value=next.backend;probe.value=null;clearRoster();nicknameEdits.value=['','',''];nicknameConfirmed.value=[false,false,false];localPlayerSlot.value='';notice.value=`已切换为 ${ocrLabels[next.backend]}，下次识别生效，重启后保留。`;});}
async function refreshNicknameDebug(){nicknameDebug.value=await invoke<NicknameDebugStatus>('nickname_debug_status');}
async function toggleNicknameDebug(){await run('nickname-debug',async()=>{nicknameDebug.value=await invoke<NicknameDebugStatus>('set_nickname_debug',{enabled:!nicknameDebug.value?.enabled});});}
watch(page,value=>{if(value==='settings')void Promise.all([refreshNicknameDebug(),refreshOcrSettings()]).catch(e=>{error.value=String(e);});});
type DisplaySettings={enabled:boolean;position:'top_center'|'bottom_center';opacity:number};
type DisplayStatus={settings:DisplaySettings;ready:boolean;error:string};
const displayStatus=ref<DisplayStatus|null>(null),displayPosition=ref<DisplaySettings['position']>('top_center'),displayOpacity=ref(85);
async function refreshDisplay(){const status=await invoke<DisplayStatus>('display_status');displayStatus.value=status;displayPosition.value=status.settings.position;displayOpacity.value=status.settings.opacity;}
async function saveDisplay(enabled=displayStatus.value?.settings.enabled??false){await run('display-settings',async()=>{displayStatus.value=await invoke<DisplayStatus>('set_display_settings',{settings:{enabled,position:displayPosition.value,opacity:Number(displayOpacity.value)}});notice.value=enabled?'游戏状态浮窗已开启，切回选定游戏窗口即可看到。':'游戏状态浮窗已关闭。';});}
async function previewDisplay(){await run('display-preview',async()=>{await invoke('set_display_settings',{settings:{enabled:displayStatus.value?.settings.enabled??false,position:displayPosition.value,opacity:Number(displayOpacity.value)}});displayStatus.value=await invoke<DisplayStatus>('preview_display');notice.value='浮窗预览15秒，鼠标操作可穿透浮窗。';});}
watch(page,value=>{if(value==='settings')void refreshDisplay().catch(e=>{error.value=String(e);});});
type AutoNextStatus={enabled:boolean;error:string};
const autoNext=ref<AutoNextStatus|null>(null);
async function refreshAutoNext(){autoNext.value=await invoke<AutoNextStatus>('auto_next_status');}
async function toggleAutoNext(){await run('auto-next',async()=>{autoNext.value=await invoke<AutoNextStatus>('set_auto_next',{enabled:!autoNext.value?.enabled});notice.value=autoNext.value.enabled?'保存后自动下一局已开启。':'保存后自动下一局已关闭。';});}
watch(page,value=>{if(value==='settings')void refreshAutoNext().catch(e=>{error.value=String(e);});});
const newRecordId=ref('');
const targets=ref<Target[]>([]),targetId=ref<number|null>(null),captureDetails=ref(false),capturePreview=ref(false);
const storedMatches=ref<Match[]>([]);
const storageStatus=ref<LocalStoreStatus|null>(null),automationStatus=ref<AutomationStatus>({running:false,stage:'waiting',phase:'等待开始自动记录',pause_reason:'',roster:'本场名单：未采集',last_saved:'',saved_count:0,processed_frames:0,interval_ms:2000,error:''});
const capture=ref<Capture>({running:false,status:'尚未启动采集',frames:0,attempts:0,width:0,height:0,interval_ms:2000});
const draft=ref<Preview|null>(null),draftPath=ref(''),kind=ref('settlement'),probe=ref<Probe|null>(null);
const rosterDraft=ref<RosterDraft|null>(null),rosterSelections=ref<string[]>(['','','']);
const nicknameEdits=ref<string[]>(['','','']),nicknameConfirmed=ref<boolean[]>([false,false,false]),localPlayerSlot=ref('');
const imageOverlay=ref<Preview|null>(null);
const demoMatches:Match[]=[{id:'demo-1',saved_at:'2026-10-31T08:30:00Z',rank:2,squad_kills:19,players:[{slot:0,nickname:'队友一',name_confirmed:true,is_local:false,kills:5,assists:3,knocks:7,damage:1520},{slot:1,nickname:'示例玩家',name_confirmed:true,is_local:true,kills:10,assists:5,knocks:10,damage:1938},{slot:2,nickname:'队友二',name_confirmed:false,is_local:false,kills:4,assists:1,knocks:4,damage:1351}]},{id:'demo-2',saved_at:'2026-10-31T07:10:00Z',rank:8,squad_kills:4,players:[{slot:0,nickname:'队友一',name_confirmed:true,is_local:false,kills:1,assists:0,knocks:1,damage:380},{slot:1,nickname:'示例玩家',name_confirmed:true,is_local:true,kills:3,assists:0,knocks:3,damage:480},{slot:2,nickname:null,name_confirmed:false,is_local:null,kills:0,assists:0,knocks:0,damage:60}]}];
const matches=computed(()=>demo.value?demoMatches:storedMatches.value);
const selected=computed(()=>matches.value.find(m=>m.id===activeId.value)??matches.value[0]??null);
const latest=computed(()=>matches.value[0]??null);
const waitingNext=computed(()=>automationStatus.value.running&&automationStatus.value.stage==='saved');
const progressItems=computed(()=>waitingNext.value?[{title:'上一局已保存',hint:'停留在结算页时不会重复识别或保存'},{title:'等待新一局画面',hint:'识别加载/选传奇画面后重新采集昵称'}]:[{title:'识别新局加载',hint:'检测左上角“三人赛”标记'},{title:'识别选传奇阶段',hint:'监视阶段，等待进入局内后读取昵称'},{title:'局内识别一次并确定名单',hint:'左下队伍昵称只识别 1 次，直接采用结果；空结果保留未知'},{title:'识别结算数据',hint:'通过小队排名和小队总击杀确认结算页'},{title:'保存战绩',hint:'保存后可在战绩记录中查看'}]);
const progressStep=computed(()=>{if(!automationStatus.value.running)return -1;if(waitingNext.value)return 1;return {waiting:-1,loading:0,legend_select:1,in_match:2,settlement:3,saving:4,saved:4}[automationStatus.value.stage];});
const target=computed(()=>targets.value.find(t=>t.handle===targetId.value));
watch(target,value=>{void invoke('remember_target',{target:value??null}).catch(()=>{});});
const titles:Record<Page,{title:string;description:string}>={home:{title:'自动记录',description:'专心打好每一局，让战绩留在这里。'},history:{title:'战绩记录',description:'回看每一场表现，也留住每一次进步。'},import:{title:'手动导入',description:'从一张截图开始，补充你的对局记录。'},settings:{title:'设置',description:'让工具按你的习惯运行。'}};
const number=(n:number|null)=>n===null?'—':n;
const field=(f:NumberField)=>f.value===null?'待核对':f.value.toLocaleString('zh-CN');
const date=(s:string)=>new Date(s).toLocaleString('zh-CN',{timeZone:'Asia/Shanghai',hour12:false,month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit'});
async function run(name:string,action:()=>Promise<void>){if(busy.value)return;busy.value=name;error.value='';notice.value='';try{await action();}catch(e){error.value=String(e);}finally{busy.value='';}}
async function refresh(){targets.value=await invoke<Target[]>('list_targets');if(!targets.value.some(t=>t.handle===targetId.value))targetId.value=targets.value.find(t=>t.title.toLowerCase().includes('apex legends'))?.handle??null;}
const refreshMatches=createRefreshQueue(async()=>({matches:await invoke<Match[]>('list_matches'),storage:await invoke<LocalStoreStatus>('local_storage_status')}),next=>{const selection=historySelection(storedMatches.value,next.matches,activeId.value);storedMatches.value=next.matches;storageStatus.value=next.storage;if(!demo.value)activeId.value=selection.selectedId;if(selection.newRecordId)newRecordId.value=selection.newRecordId;});
let historyNeedsRefresh=false;
function applyAutomationStatus(next:AutomationStatus){const previous=automationStatus.value;automationStatus.value=next;if(next.saved_count!==previous.saved_count||next.last_saved!==previous.last_saved){historyNeedsRefresh=true;void refreshLiveHistory();}}
async function refreshLiveHistory(){if(document.hidden||!historyNeedsRefresh)return;historyNeedsRefresh=false;try{await refreshMatches();}catch(e){historyNeedsRefresh=true;error.value=String(e);}}
const refreshCapture=createRefreshQueue(()=>invoke<Capture>('capture_snapshot',{preview:!document.hidden&&capturePreview.value&&captureDetails.value&&page.value==='home'}),next=>{if(document.hidden||!capturePreview.value||!captureDetails.value||page.value!=='home')next.preview=null;capture.value=next;});
async function poll(){if(document.hidden)return;try{await refreshCapture();}catch(e){error.value=String(e);}}
let statusEvents=0;
async function restoreSnapshot(){const version=statusEvents;try{const next=await invoke<AutomationStatus>('auto_record_status');if(version===statusEvents)applyAutomationStatus(next);await poll();await refreshLiveHistory();}catch(e){error.value=String(e);}}
function onVisibility(){updatePreviewTimer();if(!document.hidden){historyNeedsRefresh=true;void restoreSnapshot();}else capture.value.preview=null;}
function viewNewRecord(){demo.value=false;activeId.value=newRecordId.value;newRecordId.value='';page.value='history';}
async function toggleCapture(){await run('capture',async()=>{if(automationStatus.value.running)throw Error('自动记录运行时不能切换采集检查');if(capture.value.running)await invoke('stop_capture');else{if(!target.value)throw Error('请先选择游戏窗口');await invoke('start_capture',{target:target.value});}await poll();});}async function toggleAutomation(){await run('automation',async()=>{if(automationStatus.value.running)await invoke('stop_auto_recording');else{if(capture.value.running)throw Error('请先停止采集检查，再开始自动记录');if(!target.value)throw Error('请先选择 Apex 窗口');await invoke('start_auto_recording',{target:target.value});}await poll();});}
function showDemo(){demo.value=!demo.value;activeId.value='';}
function viewLatest(){activeId.value=latest.value?.id??'';newRecordId.value='';page.value='history';}
async function chooseImage(){await run('image',async()=>{const path=await open({multiple:false,filters:[{name:'结算或局内截图',extensions:['png','jpg','jpeg','bmp']}]});if(!path)return;const next=await invoke<Preview>('image_preview',{path});draft.value=next;draftPath.value=path;probe.value=null;});}
async function recognize(){await run('ocr',async()=>{const next=await invoke<Probe>('probe_image',{path:draftPath.value,kind:kind.value});probe.value=next;if(kind.value==='roster'){rosterDraft.value={path:draftPath.value,members:next.windows_text.map((nickname,index)=>({position:['队友 1','队友 2','本人'][index]||`队员 ${index+1}`,nickname}))};rosterSelections.value=['','',''];}if(kind.value==='settlement'&&next.settlement){nicknameEdits.value=['','',''];nicknameConfirmed.value=[false,false,false];localPlayerSlot.value='';}});}
function chooseRosterName(slot:number,position:string){rosterSelections.value[slot]=position;const member=rosterDraft.value?.members.find(m=>m.position===position);if(member)nicknameEdits.value[slot]=member.nickname||'';}
function clearRoster(){rosterDraft.value=null;rosterSelections.value=['','',''];}
async function saveRecognizedSettlement(){await run('save',async()=>{const parsed=probe.value?.settlement;if(!parsed)throw Error('请先识别结算截图');const annotations:PlayerAnnotation[]=parsed.players.map((player,index)=>{const name=nicknameEdits.value[index]?.trim()||null;const confirmed=nicknameConfirmed.value[index]||false;if(confirmed&&!name)throw Error(`请先填写${player.position}的昵称`);const isLocal=localPlayerSlot.value===''?null:Number(localPlayerSlot.value)===index;return{nickname:name,nickname_source:'manual',name_confirmed:confirmed,is_local:isLocal};});const result=await invoke<SaveMatchResult>('save_settlement',{path:draftPath.value,annotations,selectionPath:rosterDraft.value?.path??null});demo.value=false;await refreshMatches();storageStatus.value=await invoke<LocalStoreStatus>('local_storage_status');activeId.value=result.id;rosterDraft.value=null;rosterSelections.value=['','',''];page.value='history';notice.value=result.deduplicated?'这张结算截图已保存过，已打开现有记录。':'战绩和结算截图已保存到本地。';});}
async function openMatchImage(matchId:string,kind:'settlement'|'selection'){await run('source',async()=>{imageOverlay.value=await invoke<Preview>('match_image',{matchId,kind});});}
async function saveFrame(){await run('save',async()=>{const path=await save({defaultPath:'apex-capture.png',filters:[{name:'PNG 图片',extensions:['png']}]});if(path)notice.value='截图已保存：'+await invoke<string>('save_capture',{path});});}
function onKey(e:KeyboardEvent){if(e.key==='Escape')imageOverlay.value=null;}
let timer:ReturnType<typeof setInterval>|undefined;
function updatePreviewTimer(){if(timer)clearInterval(timer);timer=undefined;if(!disposed&&!document.hidden&&captureDetails.value&&page.value==='home')timer=setInterval(poll,2000);}
let unlistenTrayError:(()=>void)|undefined;
let unlistenAutomation:(()=>void)|undefined;
let disposed=false;
onMounted(async()=>{document.addEventListener('keydown',onKey);document.addEventListener('visibilitychange',onVisibility);unlistenTrayError=await listen<string>('tray-action-error',event=>{error.value=event.payload;});unlistenAutomation=await listen<AutomationStatus>('auto-record-status',event=>{statusEvents++;applyAutomationStatus(event.payload);void poll();});if(disposed){unlistenTrayError();unlistenAutomation();return;}await run('refresh',refresh);await run('storage',refreshMatches);await run('ocr-settings',refreshOcrSettings);await restoreSnapshot();updatePreviewTimer();});
onBeforeUnmount(()=>{disposed=true;document.removeEventListener('keydown',onKey);document.removeEventListener('visibilitychange',onVisibility);if(timer)clearInterval(timer);unlistenTrayError?.();unlistenAutomation?.();});
watch([captureDetails,capturePreview,page],()=>{capture.value.preview=null;updatePreviewTimer();if(captureDetails.value&&page.value==='home')void poll();});
watch(activeId,id=>{if(id===newRecordId.value)newRecordId.value='';});
</script>

<template>
<div class="app-shell">
  <aside class="sidebar">
    <div class="brand"><span class="brand-logo">A</span><div>BETTER APEX<small>每一局，都有记录</small></div></div>
    <p class="nav-label">我的空间</p>
    <nav aria-label="主导航"><button v-for="item in [{id:'home',icon:'home',name:'自动记录'},{id:'history',icon:'history',name:'战绩记录'},{id:'import',icon:'upload',name:'手动导入'},{id:'settings',icon:'settings',name:'设置'}]" :key="item.id" :class="{active:page===item.id}" :aria-current="page===item.id?'page':undefined" @click="page=item.id as Page"><Icon :name="item.icon"/><span>{{item.name}}</span><span v-if="item.id==='history'&&demo&&matches.length" class="nav-count">示例</span></button></nav>
    <div class="sidebar-note"><Icon name="monitor"/><div>本地保存<small>记录留在你的电脑</small></div></div>
    <div class="sidebar-status"><i :class="{live:automationStatus.running||capture.running}"></i><div>{{automationStatus.running?'自动记录运行中':capture.running?'采集检查运行中':'后台监测未开启'}}<small>{{automationStatus.running?automationStatus.phase:'Rust + Tauri · v0.2.0'}}</small></div></div>
  </aside>
  <div class="workspace">
    <header class="topbar"><span>我的空间 <span class="slash">/</span> {{titles[page].title}}</span><div class="topbar-right"><span v-if="demo" class="tag demo">示例数据 · 不会保存</span><span class="tag">本地模式</span><span class="avatar">A</span></div></header>
    <main>
      <div class="page-heading"><div><h1>{{titles[page].title}}</h1><p>{{titles[page].description}}</p></div></div>
      <div v-if="page==='home'" class="migration-note"><Icon name="info"/><span><strong>本地自动记录已接入</strong>选传奇页只监视阶段，局内昵称只识别 1 次并直接采用结果；结算页识别完成后保存到战绩记录。</span><button @click="page='settings'">运行说明 <Icon name="arrow"/></button></div>
      <div v-if="error" class="toast error" role="alert"><span>{{error}}</span><button aria-label="关闭提示" @click="error=''"><Icon name="close"/></button></div>
      <div v-if="notice" class="toast" role="status"><span>{{notice}}</span><button aria-label="关闭提示" @click="notice=''"><Icon name="close"/></button></div>

      <section v-show="page==='home'">
        <p class="muted">昵称只取局内画面：每局只识别 1 次，直接采用结果，空结果保留未知；选传奇阶段不读取昵称。</p>
        <div class="hero panel"><div class="hero-copy"><span class="eyebrow">YOUR NEXT MATCH</span><h2>进入游戏，安心开打。</h2><p>开始后自动识别本场队伍，在结算页记录战绩。<br>全程在后台工作，查看历史也无需停止。</p><div class="hero-actions"><button class="button primary" :disabled="!!busy||(!automationStatus.running&&(!target||capture.running))" @click="toggleAutomation"><Icon :name="automationStatus.running?'close':'play'"/>{{automationStatus.running?'停止自动记录':'开始自动记录'}}</button><button class="text-button" @click="captureDetails=!captureDetails">{{captureDetails?'收起采集检查':'检查游戏画面'}} <Icon name="chevron"/></button></div><small class="pending-text">常规每 2 秒采集 1 帧；局内昵称仅识别 1 次，结算确认时每秒最多 1 帧。</small></div><div class="hero-art" aria-hidden="true"><div class="orbit o1"></div><div class="orbit o2"></div><div class="apex-symbol">A</div><span class="art-caption">PLAY. IMPROVE. REPEAT.</span></div></div>
        <div class="connection-strip"><div><i :class="{live:!!target}"></i><strong>{{target?'已发现游戏窗口':'等待 Apex 启动'}}</strong><span>{{target?target.title:'启动游戏后可以重新检测'}}</span></div><select v-model="targetId" aria-label="目标游戏窗口" :disabled="capture.running||automationStatus.running||!!busy"><option :value="null">选择目标窗口</option><option v-for="t in targets" :key="t.handle" :value="t.handle">{{t.title}}</option></select><button class="text-button" :disabled="capture.running||automationStatus.running||!!busy" @click="run('refresh',refresh)"><Icon name="refresh"/>重新检测</button></div>
        <div class="panel automation-status"><div><span class="eyebrow">AUTOMATION</span><h3>{{automationStatus.phase}}</h3><p v-if="automationStatus.pause_reason" role="status">{{automationStatus.pause_reason}}</p><p>{{automationStatus.roster}}</p><p v-if="automationStatus.last_saved">{{automationStatus.last_saved}} · 已保存 {{automationStatus.saved_count}} 场</p><p v-if="automationStatus.error" class="automation-error">{{automationStatus.error}}</p></div><span class="tag">{{automationStatus.running?`DXGI · 每 ${automationStatus.interval_ms/1000} 秒最多 1 帧`:'本地 · 不自动上传'}}</span></div><div v-if="captureDetails" class="panel diagnostic"><div class="panel-title"><h3>画面采集检查</h3><span class="tag">不自动保存战绩</span></div><p class="muted">采集间隔保持 2 秒。开始后切回游戏，返回这里可查看最近一次有效画面。</p><div class="inline-form"><select v-model="targetId" aria-label="目标窗口" :disabled="capture.running||!!busy"><option :value="null">选择游戏窗口</option><option v-for="t in targets" :key="t.handle" :value="t.handle">{{t.title}}</option></select><button class="button secondary" :disabled="!!busy||(!capture.running&&!targetId)" @click="toggleCapture">{{capture.running?'停止检查':'开始采集检查'}}</button></div><div class="diagnostic-footer"><span>{{capture.status}}</span><label><input v-model="capturePreview" type="checkbox" @change="poll">显示预览</label><button class="text-button" :disabled="!capture.width||!!busy" @click="saveFrame">保存截图</button></div><div v-if="capturePreview" class="capture-image"><img v-if="capture.preview" :src="capture.preview" alt="最近一次采集的画面"><p v-else>等待有效画面</p></div></div>
        <div class="home-grid"><div class="panel progress-panel"><div class="panel-title"><h3>{{waitingNext?'上一局已保存':'本场进度'}}</h3><span class="quiet-label">{{waitingNext?'等待新一局':automationStatus.running?`${automationStatus.processed_frames} 帧已检查`:'尚未运行'}}</span></div><ol class="steps"><li v-for="(step,i) in progressItems" :key="step.title" :class="{complete:i<progressStep,current:i===progressStep,pending:progressStep<0||i>progressStep}"><span>{{i<progressStep?'✓':i+1}}</span><div><strong>{{step.title}}</strong><small>{{i<progressStep?'已完成':i===progressStep?waitingNext?'等待检测到新一局画面':automationStatus.phase:automationStatus.running?step.hint:'开始自动记录后自动更新'}}</small></div></li></ol></div><div class="panel recent-panel"><div class="panel-title"><h3>最近一场</h3><button class="text-button" @click="page='history'">全部记录 <Icon name="arrow"/></button></div><template v-if="latest"><div class="recent-rank"><small>{{demo?'示例战绩':'最近保存'}}</small><strong>#{{number(latest.rank)}}</strong><span>小队总击杀 {{number(latest.squad_kills)}}</span></div><div class="recent-names">{{latest.players.map(p=>p.nickname||'战绩归属待核对').join(' / ')}}</div><div class="recent-bottom"><span>{{date(latest.saved_at)}} · 保存时间</span><button class="text-button" @click="viewLatest">查看详情 <Icon name="arrow"/></button></div></template><div v-else class="empty compact"><Icon name="history"/><strong>等待第一条战绩</strong><p>保存后的对局会显示在这里；也可从“手动导入”补录截图。</p></div></div></div>
      </section>

      <section v-show="page==='history'">
        <div class="section-toolbar"><div class="segmented"><button class="selected">全部战绩</button></div><div class="history-actions"><button v-if="newRecordId" class="text-button" @click="viewNewRecord">查看新记录</button><span class="muted">{{demo?'示例布局':matches.length+' 场记录'}}</span><button class="text-button" :disabled="!!busy" @click="demo?showDemo():run('history',refreshMatches)">{{demo?'退出示例':'刷新记录'}}</button><button v-if="!demo" class="text-button" @click="showDemo">查看示例效果</button></div></div>
        <div v-if="!matches.length" class="panel empty large"><Icon name="history"/><h2>还没有战绩</h2><p>保存后的对局会显示在这里；也可从“手动导入”补录截图。<br>旧版战绩不会导入新版。</p><button class="button primary" @click="showDemo">先看看界面示例 <Icon name="arrow"/></button></div>
        <div v-else class="history-layout"><div class="match-list" aria-label="战绩列表"><button v-for="m in matches" :key="m.id" class="match-item" :class="{selected:selected?.id===m.id}" @click="activeId=m.id"><div class="placement" :class="{podium:m.rank!==null&&m.rank<=3}">#{{number(m.rank)}}</div><div><strong>{{m.rank===1?'捍卫者':'比赛总结'}}</strong><small>{{date(m.saved_at)}} · 保存时间</small><span>{{m.players.map(p=>p.nickname||'待核对').join(' / ')}}</span></div><span class="kill-count">{{number(m.squad_kills)}}<small>小队击杀</small></span></button></div><article v-if="selected" class="panel match-detail"><div class="match-detail-heading"><div><span class="eyebrow">{{demo?'DEMO MATCH':'MATCH RECORD'}}</span><h2>第 {{number(selected.rank)}} 名 <span>小队总击杀 {{number(selected.squad_kills)}}</span></h2><p>{{date(selected.saved_at)}} · 北京时间 · 保存时间</p></div><span class="tag" :class="{demo}">{{demo?'示例记录':'本地记录'}}</span></div><div class="table-wrap"><table><thead><tr><th>玩家</th><th>击杀</th><th>助攻</th><th>击倒</th><th>伤害</th></tr></thead><tbody><tr v-for="p in selected.players" :key="p.slot" :class="{self:p.is_local}"><td><div class="player-name">{{p.nickname||'战绩归属待核对'}}<span v-if="p.is_local" class="self-label">本人</span></div><small>{{p.name_confirmed?'关联已确认':'战绩归属待核对'}}</small></td><td>{{number(p.kills)}}</td><td>{{number(p.assists)}}</td><td>{{number(p.knocks)}}</td><td class="damage">{{p.damage===null?'待核对':number(p.damage)}}</td></tr></tbody></table></div><div class="evidence"><div class="panel-title"><h3>来源截图</h3><div class="evidence-actions"><button v-if="selected.has_selection_image" class="text-button" :disabled="!!busy" @click="openMatchImage(selected.id,'selection')">查看昵称来源图</button><button v-if="selected.has_settlement_image" class="text-button" :disabled="!!busy" @click="openMatchImage(selected.id,'settlement')">查看结算图</button><span v-if="!selected.has_settlement_image&&!selected.has_selection_image" class="quiet-label">示例记录无来源图</span></div></div><p class="muted">{{selected.has_settlement_image?'原始截图保存在本机。':'示例记录不包含实际截图。'}}</p></div></article></div>
      </section>

      <section v-show="page==='import'">
        <div class="import-flow"><span class="current">01 导入截图</span><Icon name="chevron"/><span :class="{current:!!probe}">02 识别结果</span><Icon name="chevron"/><span :class="{current:!!probe&&kind==='settlement'}">03 核对关联并保存</span></div>
        <div class="import-layout">
          <div class="panel import-source">
            <div class="panel-title"><h3>截图</h3><button v-if="draft" class="text-button" :disabled="!!busy" @click="chooseImage">更换图片</button></div>
            <div v-if="!draft" class="upload-area"><div class="upload-icon"><Icon name="upload"/></div><h2>添加一张游戏截图</h2><p>支持 PNG、JPG、BMP<br>使用完整的结算页或局内画面</p><button class="button primary" :disabled="!!busy" @click="chooseImage">选择截图</button></div>
            <template v-else><button class="draft-image" aria-label="在窗口内放大截图" @click="imageOverlay=draft"><img :src="draft.image" :alt="draft.name"/></button><div class="file-caption"><strong>{{draft.name}}</strong><small>{{draft.width}} × {{draft.height}} · 点击图片放大</small></div></template>
          </div>
          <div class="panel import-result">
            <h3>识别选项</h3><label class="field-label" for="recognition-kind">这是什么画面？</label>
            <select id="recognition-kind" v-model="kind" :disabled="!!busy" @change="probe=null"><option value="settlement">三人结算页</option><option value="roster">局内队伍昵称</option></select>
            <p class="muted">{{kind==='roster'?'读取局内左下三人昵称；本次只测试一张图片，结果暂存供同局结算手动关联。':'结算图识别排名、三人击杀／助攻／击倒／伤害；未核对的昵称和本人身份会保留为待核对。'}}</p>
            <button class="button primary full" :disabled="!draft||!!busy" @click="recognize">{{busy==='ocr'?'正在识别…':'识别截图'}}</button>
            <template v-if="probe&&kind==='settlement'&&probe.settlement">
              <div class="result-heading">三人战绩 · 请核对后续关联</div><div class="settlement-summary"><div><small>小队排名</small><strong>{{field(probe.settlement.rank)}}</strong></div><div><small>小队总击杀</small><strong>{{field(probe.settlement.squad_kills)}}</strong></div></div>
              <div v-for="(player,index) in probe.settlement.players" :key="player.position" class="parsed-player"><div class="parsed-player-title"><strong>{{nicknameEdits[index]||player.position}}</strong><span>归属待核对</span></div><p>击杀 {{field(player.kills)}} / 助攻 {{field(player.assists)}} / 击倒 {{field(player.knocks)}}</p><strong class="parsed-damage">伤害 {{field(player.damage)}}</strong></div>
              <ul v-if="probe.settlement.warnings.length" class="parse-warnings"><li v-for="warning in probe.settlement.warnings" :key="warning">{{warning}}</li></ul>
              <div class="association-panel"><div class="panel-title"><h3>{{rosterDraft?'关联本场昵称':'核对玩家信息'}}</h3><button v-if="rosterDraft" class="text-button" @click="clearRoster">清除名单</button></div><p class="muted">{{rosterDraft?'选择对应的局内队员并核对昵称。勾选“已核对”后才会将昵称标为确认。':'结算图不提供最终昵称；请选择同场名单或手动填写已核对的昵称，并确认本人位置。'}}</p><div v-for="(player,index) in probe.settlement.players" :key="player.position+'-association'" class="association-member"><strong>{{player.position}}</strong><label v-if="rosterDraft">局内队员<select v-model="rosterSelections[index]" @change="chooseRosterName(index,rosterSelections[index])"><option value="">不关联</option><option v-for="member in rosterDraft.members" :key="member.position" :value="member.position">{{member.position}} · {{member.nickname||'未识别'}}</option></select></label><span v-else class="association-pending">尚未暂存局内名单</span><label>保存昵称<input v-model="nicknameEdits[index]" maxlength="80" placeholder="可填写或修正昵称"></label><label class="confirm-name"><input v-model="nicknameConfirmed[index]" type="checkbox">已核对</label></div><label class="local-player">本人位置<select v-model="localPlayerSlot"><option value="">暂不确认</option><option v-for="(player,index) in probe.settlement.players" :key="player.position+'-local'" :value="String(index)">{{player.position}} · {{nicknameEdits[index]||player.position}}</option></select></label></div>
              <button class="button secondary full save-match" :disabled="!!busy" @click="saveRecognizedSettlement">{{busy==='save'?'正在保存…':rosterDraft?'保存关联与战绩':'保存本地战绩'}}</button>
            </template>
            <template v-else-if="probe"><div class="result-heading">局内昵称结果</div><div v-for="(text,i) in probe.windows_text" :key="i" class="result-row"><small>{{['队友 1','队友 2','本人'][i]}}</small><strong>{{text||'待核对'}}</strong></div><p class="muted">本次局内名单已暂存；导入同一局结算图后，可选择队员并核对战绩归属。手动测试只读取当前图片一次。</p><div v-if="rosterDraft" class="roster-staged"><span>本场名单：{{rosterDraft.members.map(m=>m.nickname||'待核对').join(' / ')}}</span><button class="text-button" @click="clearRoster">清除</button></div></template>
            <div v-else class="result-placeholder"><Icon name="image"/><span>导入截图后查看识别结果</span></div>
          </div>
        </div>
      </section>

      <section v-show="page==='settings'" class="settings-layout">
        <div class="panel settings-card"><h3>自动下一局</h3><div class="setting-row"><div><strong>保存结算后自动按数字1</strong><small>默认开启。仅在本局数字完整、自动保存成功，并确认游戏前台仍有“重新匹配”提示时，按一次主键盘数字1。每局最多触发一次。</small></div><button class="switch" :class="{on:autoNext?.enabled}" role="switch" :aria-checked="autoNext?.enabled??false" aria-label="保存后自动下一局" :disabled="!!busy||!autoNext" @click="toggleAutoNext"><span></span></button></div><p v-if="autoNext?.error" class="automation-error">{{autoNext.error}}</p><p class="muted">可随时关闭，重启保留。切出游戏、停止记录、画面已变化、按住修饰键，或10秒内未取得新的确认画面时跳过，请手动继续；不会切换窗口或连续按键。手动导入、历史恢复、重复保存及含待核对数字的战绩不会触发。浮窗会提示按键是否已发送，实际进入匹配仍由游戏决定。</p></div>
        <div class="panel settings-card"><h3>游戏状态浮窗 · Display</h3><div class="setting-row"><div><strong>在游戏画面上显示工具状态</strong><small>显示当前记录阶段、暂停或重试状态，以及 Better Apex 自身 CPU 和专用内存占用；包含 WebView2 子进程，每2秒更新。</small></div><button class="switch" :class="{on:displayStatus?.settings.enabled}" role="switch" :aria-checked="displayStatus?.settings.enabled??false" aria-label="游戏状态浮窗" :disabled="!!busy||!displayStatus?.ready" @click="saveDisplay(!displayStatus?.settings.enabled)"><span></span></button></div><div class="inline-form"><label>浮窗位置<select v-model="displayPosition" :disabled="!!busy"><option value="top_center">上方居中</option><option value="bottom_center">下方居中</option></select></label><label>不透明度 {{displayOpacity}}%<input v-model.number="displayOpacity" type="range" min="40" max="100" step="5" :disabled="!!busy" aria-label="浮窗不透明度"></label><button class="button secondary" :disabled="!!busy||!displayStatus?.ready" @click="saveDisplay()">保存浮窗设置</button><button class="button secondary" :disabled="!!busy||!displayStatus?.ready" @click="previewDisplay">预览15秒</button></div><p v-if="displayStatus?.error" class="automation-error">{{displayStatus.error}}</p><p class="muted">默认关闭，开启后仅在选定游戏位于前台时显示，切出或最小化自动隐藏；浮窗不抢焦点，鼠标可穿透。适用于窗口化／无边框窗口，独占全屏可能不可见。CPU按整机总算力计为0–100%；专用内存为本软件进程树的提交量，与任务管理器默认内存列口径不同。浮窗不增加截图或OCR频率。</p></div>
        <div class="panel settings-card"><h3>OCR 引擎</h3><div class="setting-row"><div><strong>识别引擎（昵称与结算）</strong><small>用于局内昵称、结算卡片匹配线索及排名／击杀／助攻／击倒／伤害。选择 PP-OCRv6 small 后，结算数字也由同一模型解析。昵称仍只取02局内单次结果；03文字只作匹配线索。</small><small>当前使用：{{ocrSettings?ocrLabels[ocrSettings.backend]:'正在读取…'}}。选择后点击保存，重启后保留。</small><small v-if="automationStatus.running">请先停止自动记录再切换，避免本局混用引擎。</small><small v-if="ocrSettings&&!ocrSettings.ppocr_ready" class="automation-error">PP-OCRv6 small 资源不完整，请保留 EXE 旁的完整 resources 目录。</small><small v-if="ocrSettings?.error" class="automation-error">{{ocrSettings.error}}</small></div></div><div class="inline-form"><select v-model="ocrChoice" aria-label="OCR 引擎" :disabled="!!busy||automationStatus.running||!ocrSettings"><option value="pp_ocr_v6_small" :disabled="!ocrSettings?.ppocr_ready">PP-OCRv6 small · 本地模型</option><option value="windows_tesseract">Windows OCR + Tesseract · 原方案</option></select><button class="button secondary" :disabled="!!busy||automationStatus.running||!ocrSettings||ocrChoice===ocrSettings.backend&&!ocrSettings.error" @click="saveOcrBackend">{{busy==='ocr-settings'?'正在加载并保存…':'保存 OCR 设置'}}</button></div><p class="muted">首次试用默认 PP-OCRv6 small，可随时切回原方案。两种引擎均在本机识别；PP-OCR模式不混用旧数字引擎，结算原始读数继续写入调试日志。切换会清空尚未保存的手动识别结果和暂存名单，请先保存需要的记录。</p></div>
        <div class="panel settings-card"><h3>临时昵称调试日志</h3><div class="setting-row"><div><strong>记录所有昵称识别样本</strong><small>记录原始读数、空结果、错误、单次识别结果与最终关联，只保存在本机；本次运行可关闭，重启后默认开启。</small></div><button class="switch" :class="{on:nicknameDebug?.enabled}" role="switch" :aria-checked="nicknameDebug?.enabled??false" aria-label="临时记录昵称样本" :disabled="!!busy||!nicknameDebug" @click="toggleNicknameDebug"><span></span></button></div><div class="setting-row"><div><strong>日志目录</strong><small class="path">{{nicknameDebug?.directory||'正在读取…'}}</small><small>每次运行首次记录时生成一组 TXT 和 JSONL 文件，旧文件不会覆盖或自动删除。</small><small v-if="nicknameDebug?.current_file" class="path">当前文件：{{nicknameDebug.current_file}}</small><small v-if="nicknameDebug?.error" class="automation-error">{{nicknameDebug.error}}</small></div><button class="button secondary" :disabled="!!busy" @click="run('debug-folder',async()=>{await invoke('open_nickname_debug_folder');await refreshNicknameDebug();})">打开日志文件夹</button></div></div>
        <div class="panel settings-card"><h3>数据与记录</h3><div class="setting-row"><div><strong>新版本地战绩库</strong><small class="path">{{storageStatus?.database_path||'正在准备独立数据目录…'}}</small><small>已接入本地保存；数据与旧版隔离，不会上传云端。</small></div><span class="setting-value">{{storageStatus?`${storageStatus.record_count} 场 · v${storageStatus.schema_version}`:'初始化中'}}</span></div><div class="setting-row"><div><strong>界面示例</strong><small>只在界面内展示，不写入正式战绩</small></div><button class="switch" :class="{on:demo}" role="switch" :aria-checked="demo" aria-label="显示界面示例" @click="showDemo"><span></span></button></div></div>
        <div class="panel settings-card"><h3>采集与运行</h3><div class="setting-row"><div><strong>画面分辨率</strong><small>完整16:9画面自动适配，无需手动切换；来源截图保留采集时的原始尺寸。</small></div><span class="setting-value">1080P / 2K</span></div><div class="setting-row"><div><strong>采集方式</strong><small>仅在选定游戏窗口位于前台时采集</small></div><span class="setting-value">DXGI · 无黄框</span></div><div class="setting-row"><div><strong>常规采集间隔</strong><small>页面切换不会启动额外采集</small></div><span class="setting-value">2 秒</span></div><div class="setting-row"><div><strong>窗口与后台</strong><small>自动记录运行时关闭主窗口会收至托盘；托盘菜单可重新打开或退出</small></div><span class="tag">单主窗口 · 托盘</span></div></div>
        <div class="panel settings-card"><h3>功能状态</h3><div class="migration-steps"><div><Icon name="check"/><span>Rust 采集与 OCR</span><small>已接入</small></div><div><Icon name="check"/><span>结算识别、昵称关联与本地历史</span><small>已接入</small></div><div><Icon name="check"/><span>局内到结算自动记录</span><small>已接入</small></div><div><Icon name="check"/><span>单窗口、托盘与单实例激活</span><small>已接入</small></div><div class="future"><Icon name="chevron"/><span>游戏电脑整体验收与性能对比</span><small>待实测</small></div></div><p class="muted">Better Apex · Rust + Tauri · v0.2.0<br>新版从空数据开始，旧版战绩不会迁入。</p></div>
      </section>
      <footer><span><i :class="{live:automationStatus.running||capture.running}"></i>{{automationStatus.running?'自动记录运行中':capture.running?'采集检查运行中':'本地模式 · 自动记录已就绪'}}</span><span>让每一局的进步，都看得见。</span></footer>
    </main>
  </div>
  <div v-if="imageOverlay" class="image-overlay" role="dialog" aria-modal="true" aria-label="来源截图" @click.self="imageOverlay=null"><div class="image-dialog"><div><span>{{imageOverlay.name}} <small>{{imageOverlay.width}} × {{imageOverlay.height}}</small></span><button class="icon-button" aria-label="关闭图片" @click="imageOverlay=null"><Icon name="close"/></button></div><img :src="imageOverlay.image" :alt="imageOverlay.name"/><small>Esc 关闭 · 此图片在主窗口内查看</small></div></div>
</div>
</template>

<style>
.settlement-summary{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px;margin:15px 0}
.settlement-summary>div,.parsed-player{background:#141d29;border:1px solid #2a384a;border-radius:8px;padding:13px}
.settlement-summary small{display:block;color:#879ab3;margin-bottom:7px}
.settlement-summary strong{font-size:19px}
.parsed-player{margin:9px 0}
.parsed-player-title{display:flex;align-items:center;justify-content:space-between;gap:12px}
.parsed-player-title strong{font-size:13px}
.parsed-player-title span{font-size:9px;color:#e2ba84;border:1px solid #66563f;border-radius:5px;padding:4px 7px}
.parsed-player p{margin:10px 0 5px;color:#bac9dc;font-size:11px}
.parsed-damage{color:#f18b75;font-size:14px}
.steps li.current>span{background:#ed806c;border-color:#ed806c;color:#141b25}.steps li.current strong{color:#f2a28e}.steps li.complete>span{background:#263e38;border-color:#477765;color:#9ed0b6}.steps li.complete strong{color:#a9c7b8}.steps li.pending{opacity:.62}
.parse-warnings{padding-left:18px;color:#d6bd91;font-size:10px;line-height:1.8}
.selection-player{margin:9px 0;padding:12px;background:#141d29;border:1px solid #2a384a;border-radius:8px}
.selection-primary{display:flex;align-items:center;gap:14px;margin-bottom:10px}
.selection-primary>strong{min-width:54px;font-size:10px;color:#aebdd1}
.selection-primary>div,.selection-evidence>div{min-width:0}
.selection-evidence{display:grid;grid-template-columns:1fr 1fr;gap:10px;padding-top:9px;border-top:1px solid #263449}
.selection-player small{display:block;margin-bottom:6px;color:#798da8}
.selection-player b{display:block;overflow-wrap:anywhere;font-size:11px;font-weight:600;color:#e1e7f0}
.automation-status{display:flex;align-items:flex-start;justify-content:space-between;gap:20px;margin:18px 0;padding:18px 20px}
.automation-status h3{margin:7px 0;color:#f0f4fa;font-size:14px}
.automation-status p{margin:6px 0;color:#93a7c1;font-size:11px}
.automation-error{color:#ff9784!important}
.connection-strip select{max-width:310px;min-width:180px;padding:9px 12px;background:#111a25;border:1px solid #304158;border-radius:8px;color:#dce6f5}
.association-panel{margin-top:16px;padding:14px;background:#141d29;border:1px solid #2a384a;border-radius:9px}
.association-panel>.panel-title{margin-bottom:8px}
.association-panel>.panel-title h3{font-size:13px}
.association-panel>.muted{margin:0 0 12px;font-size:10px;line-height:1.6}
.association-member{display:grid;grid-template-columns:70px minmax(0,1fr) minmax(0,1fr);align-items:end;gap:8px;padding:10px 0;border-top:1px solid #263449}
.association-member>strong{align-self:center;font-size:11px;color:#aebdd1}
.association-member label,.local-player{display:flex;flex-direction:column;gap:5px;color:#8498b2;font-size:9px}
.association-member select,.association-member input:not([type=checkbox]),.local-player select{width:100%;min-width:0;padding:8px 9px;background:#101823;border:1px solid #34445a;border-radius:6px;color:#e3eaf4;font:inherit;font-size:10px}
.association-member .confirm-name{flex-direction:row;align-items:center;grid-column:2/-1;color:#b8c6d9}
.association-pending{grid-column:2/-1;align-self:center;color:#7186a2;font-size:9px}
.local-player{margin-top:12px;max-width:240px}
.roster-staged{display:flex;justify-content:space-between;align-items:center;gap:12px;margin-top:12px;padding:10px 12px;border:1px solid #31534f;background:#152a2b;border-radius:8px;color:#b3d6ce;font-size:10px}
.evidence-actions{display:flex;align-items:center;gap:14px}
.save-match{margin-top:14px}
@media(max-width:640px){.selection-evidence{grid-template-columns:1fr}.association-member{grid-template-columns:54px minmax(0,1fr)}.association-member label,.association-member .association-pending{grid-column:2}.automation-status{flex-direction:column}.connection-strip select{max-width:100%;min-width:120px}}
</style>
