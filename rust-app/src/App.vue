<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
type Target = { handle:number; pid:number; title:string };
type Capture = { running:boolean; status:string; frames:number; attempts:number; width:number; height:number; interval_ms:number; preview?:string|null };
type Probe = { width:number; height:number; windows_text:string[]; tesseract_text:string[]; scope:string };
type Player = { slot:number; nickname:string|null; name_confirmed:boolean; is_local:boolean|null; kills:number|null; assists:number|null; knocks:number|null; damage:number|null };
type Match = { id:string; saved_at:string; rank:number|null; squad_kills:number|null; players:Player[] };
type Database = { schema:number; count:number; read_only:boolean; matches:Match[] };
const targets=ref<Target[]>([]), selected=ref<number|null>(null);
const capture=ref<Capture>({running:false,status:'尚未开始采集验证',frames:0,attempts:0,width:0,height:0,interval_ms:2000});
const busy=ref(''), error=ref(''), notice=ref(''),preview=ref(false),tab=ref('capture');
const kind=ref('settlement'),ocr=ref<Probe|null>(null),db=ref<Database|null>(null),activeMatch=ref<Match|null>(null),sourceName=ref('');
async function task(name:string,action:()=>Promise<void>){if(busy.value)return;busy.value=name;error.value='';notice.value='';try{await action();}catch(e){error.value=String(e);}finally{busy.value='';}}
async function refresh(){targets.value=await invoke<Target[]>('list_targets');if(!targets.value.some(t=>t.handle===selected.value))selected.value=targets.value.find(t=>t.title.toLowerCase().includes('apex legends'))?.handle??null;}
async function poll(){if(document.hidden)return;try{capture.value=await invoke<Capture>('capture_snapshot',{preview:preview.value&&tab.value==='capture'});}catch(e){error.value=String(e);}}
async function toggle(){await task('capture',async()=>{if(capture.value.running)await invoke('stop_capture');else{const target=targets.value.find(t=>t.handle===selected.value);if(!target)throw new Error('请先选择目标窗口');await invoke('start_capture',{target});}await poll();});}
async function imageProbe(){await task('ocr',async()=>{const path=await open({multiple:false,filters:[{name:'原始截图',extensions:['png','jpg','jpeg','bmp']}]});if(!path)return;sourceName.value=path.split(/[\\/]/).pop()??path;ocr.value=await invoke<Probe>('probe_image',{path,kind:kind.value});});}
async function databaseProbe(){await task('db',async()=>{const path=await open({multiple:false,filters:[{name:'旧版数据库',extensions:['db']}]});if(!path)return;db.value=await invoke<Database>('inspect_database',{path});activeMatch.value=db.value.matches[0]??null;});}
async function saveFrame(){await task('save',async()=>{const path=await save({defaultPath:'apex-rust-capture.png',filters:[{name:'PNG',extensions:['png']}]});if(path)notice.value='已保存：'+await invoke<string>('save_capture',{path});});}
const number=(value:number|null)=>value===null?'待核对':value;
const time=(value:string)=>new Date(value).toLocaleString('zh-CN',{timeZone:'Asia/Shanghai',hour12:false});
let timer:ReturnType<typeof setInterval>|undefined;
onMounted(async()=>{await task('refresh',refresh);timer=setInterval(poll,2000);});
onBeforeUnmount(()=>{if(timer)clearInterval(timer);});
</script>

<template>
  <div class="shell">
    <aside><div class="brand"><span class="brand-mark">A</span><div>BETTER APEX<small>RUST COMPATIBILITY LAB</small></div></div>
      <div class="stage">迁移步骤 A <span>兼容性验证</span></div>
      <nav aria-label="验证项目"><button v-for="item in [{id:'capture',name:'画面采集',icon:'◉'},{id:'ocr',name:'识别通路',icon:'▤'},{id:'data',name:'旧数据读取',icon:'▦'}]" :key="item.id" :class="{selected:tab===item.id}" @click="tab=item.id"><span>{{item.icon}}</span>{{item.name}}</button></nav>
      <div class="sidebar-bottom"><i :class="{live:capture.running}"></i>{{capture.running?'采集验证运行中':'尚未开始采集'}}<small>页面切换不重建采集任务</small></div>
    </aside>
    <main>
      <header><div><p class="eyebrow">BETTER APEX / MIGRATION</p><h1>{{tab==='capture'?'先确认，能看见游戏':tab==='ocr'?'验证原生识别通路':'让旧战绩继续可用'}}</h1><p class="subtitle">{{tab==='capture'?'Rust 直接调用 DXGI，验证客户区画面与低频采集。':tab==='ocr'?'沿用现有模型，验证 Windows OCR 与 Tesseract 原生库。':'以只读方式打开旧版数据库，查看原有字段和记录。'}}</p></div><span class="version">阶段 A · v0.1.0</span></header>
      <div class="scope"><strong>这是迁移验证版</strong> 自动记录、完整识别解析和正式 UI 尚未迁移，请继续用旧版记录游戏。</div>
      <div v-if="error" class="message error" role="alert">{{error}}</div><div v-if="notice" class="message">{{notice}}</div>
      <section v-show="tab==='capture'">
        <div class="card control"><div class="status-line"><i :class="{live:capture.running}"></i><strong>{{capture.status}}</strong><span class="pill">每 2 秒最多 1 帧</span></div>
          <div class="actions"><select v-model="selected" :disabled="capture.running||!!busy" aria-label="目标窗口"><option :value="null">选择 Apex 或测试窗口</option><option v-for="target in targets" :key="target.handle" :value="target.handle">{{target.title}}</option></select><button class="secondary" :disabled="capture.running||!!busy" @click="task('refresh',refresh)">重新检测</button><button class="primary" :disabled="!!busy||(!capture.running&&!selected)" @click="toggle">{{capture.running?'停止采集验证':'开始采集验证'}}</button></div>
          <p class="hint">开始后切回目标窗口前台；切出、最小化或关键区域被遮挡时暂停。</p>
        </div>
        <div class="metrics"><div><small>累计有效画面</small><strong>{{capture.frames}}<em> 帧</em></strong></div><div><small>采集尝试</small><strong>{{capture.attempts}}<em> 次</em></strong></div><div><small>原始客户区</small><strong>{{capture.width?`${capture.width} × ${capture.height}`:'等待画面'}}</strong></div></div>
        <div class="card"><div class="section-head"><h2>画面预览</h2><label><input v-model="preview" type="checkbox" @change="poll"> 展开预览</label></div><div v-if="preview" class="preview"><img v-if="capture.preview" :src="capture.preview" alt="选定窗口客户区"><p v-else>等待目标窗口前台的新画面</p></div><p v-else class="hint">默认关闭缩略图编码与传输。采集仍按固定低频执行。</p><button class="secondary" :disabled="!capture.width||!!busy" @click="saveFrame">保存已有画面的原始 PNG</button></div>
      </section>
      <section v-show="tab==='ocr'"><div class="card"><h2>选择一张完整原图</h2><p class="hint">结算图验证三人的伤害数字；局内图验证昵称区域。此阶段不保存战绩。</p><div class="actions"><select v-model="kind" :disabled="!!busy" aria-label="验证类型"><option value="settlement">结算伤害区域</option><option value="roster">局内昵称区域</option></select><button class="primary" :disabled="!!busy" @click="imageProbe">{{busy==='ocr'?'正在验证…':'选择图片并验证'}}</button></div></div>
        <div v-if="ocr" class="card result"><div class="section-head"><h2>{{sourceName}}</h2><span>{{ocr.width}} × {{ocr.height}}</span></div><p class="hint">{{ocr.scope}}</p><div v-if="ocr.tesseract_text.length" class="metrics"><div v-for="(text,i) in ocr.tesseract_text" :key="i"><small>{{['左侧伤害','中间伤害','右侧伤害'][i]}}</small><strong>{{text||'未读到'}}</strong></div></div><div v-else class="names"><div v-for="(text,i) in ocr.windows_text" :key="i"><small>{{['队友 1 区域','队友 2 区域','本人区域'][i]}}</small><strong>{{text||'未读到'}}</strong></div></div><details v-if="ocr.tesseract_text.length"><summary>查看 Windows OCR 通路返回</summary><p v-for="text in ocr.windows_text" :key="text">{{text}}</p></details></div>
      </section>
      <section v-show="tab==='data'"><div class="card"><div class="section-head"><div><h2>读取旧版 matches.db</h2><p class="hint">不会创建、升级或写入数据库。建议先使用完整 data 目录的副本。</p></div><button class="primary" :disabled="!!busy" @click="databaseProbe">选择数据库</button></div></div>
        <template v-if="db"><p class="data-count">共 {{db.count}} 场 · 结构 v{{db.schema}} · 只读 · 最多展示最近 50 场</p><div class="history"><div class="record-list"><button v-for="match in db.matches" :key="match.id" :class="{active:activeMatch?.id===match.id}" @click="activeMatch=match"><strong>第 {{number(match.rank)}} 名 <span>小队击杀 {{number(match.squad_kills)}}</span></strong><small>{{time(match.saved_at)}}</small></button><p v-if="!db.matches.length" class="hint">数据库中暂无战绩</p></div><div class="card detail" v-if="activeMatch"><h2>第 {{number(activeMatch.rank)}} 名 · 小队击杀 {{number(activeMatch.squad_kills)}}</h2><p class="hint">保存时间：{{time(activeMatch.saved_at)}}（北京时间）</p><table><thead><tr><th>玩家</th><th>击杀</th><th>助攻</th><th>击倒</th><th>伤害</th></tr></thead><tbody><tr v-for="p in activeMatch.players" :key="p.slot"><td>{{p.nickname||'昵称待核对'}}<small>{{p.is_local?'本人 · ':''}}{{p.name_confirmed?'已关联':'待核对'}}</small></td><td>{{number(p.kills)}}</td><td>{{number(p.assists)}}</td><td>{{number(p.knocks)}}</td><td>{{number(p.damage)}}</td></tr></tbody></table></div></div></template>
      </section>
      <footer>本地运行 · Rust + Tauri · 不包含 .NET 后台 <span>兼容性验证阶段，不代表游戏性能已验收</span></footer>
    </main>
  </div>
</template>
