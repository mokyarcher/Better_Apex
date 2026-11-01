// Owned-app E2E through this application's dedicated local WebView2 CDP port.
// Does not connect to the user's browser or select/capture another app.
import {spawn} from 'node:child_process';
import {createServer} from 'node:net';
import {mkdir,readFile,writeFile,copyFile} from 'node:fs/promises';
import path from 'node:path';
const [exe,output,repo]=process.argv.slice(2).map(p=>path.resolve(p));
await mkdir(output,{recursive:true});
const db=path.join(output,'legacy-copy.db');await copyFile(path.join(repo,'data','matches.db'),db);
const server=createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
const child=spawn(exe,[],{cwd:path.dirname(exe),windowsHide:true,stdio:['ignore','pipe','pipe'],env:{...process.env,WEBVIEW2_USER_DATA_FOLDER:path.join(output,'webview-profile'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`}});
let logs='';child.stdout.on('data',d=>logs+=d);child.stderr.on('data',d=>logs+=d);
let exited=false;child.on('exit',()=>exited=true);
const delay=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label,timeout=20000){const start=Date.now();while(true){if(await fn())return;if(Date.now()-start>timeout)throw Error('Timeout: '+label);await delay(100);}}
let ws;const pending=new Map();let id=0;
async function call(method,params={}){return new Promise((resolve,reject)=>{const key=++id;pending.set(key,{resolve,reject});ws.send(JSON.stringify({id:key,method,params}));});}
async function evaluate(expression){const r=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw Error(JSON.stringify(r.exceptionDetails));return r.result?.value;}
const invoke=(name,args={})=>evaluate(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(name)},${JSON.stringify(args)})`);
const click=async text=>{
  const expression=`[...document.querySelectorAll('button')].find(b=>b.textContent.trim().endsWith(${JSON.stringify(text)}))`;
  await until(()=>evaluate(`(()=>{const b=${expression};return !!b&&!b.disabled})()`),'button ready: '+text);
  return evaluate(`(()=>{const b=${expression};b.click();return true})()`);
};
const check=(v,msg)=>{if(!v)throw Error(msg);};
const results=[];
try{
  let page;
  await until(async()=>{if(exited)throw Error('Application exited: '+logs);try{page=(await(await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(p=>p.type==='page'&&p.url.includes('tauri.localhost'));return !!page;}catch{return false;}},'owned WebView2 page');
  ws=new WebSocket(page.webSocketDebuggerUrl);await new Promise((r,j)=>{ws.addEventListener('open',r,{once:true});ws.addEventListener('error',j,{once:true});});
  ws.addEventListener('message',event=>{const m=JSON.parse(event.data);if(!m.id)return;const request=pending.get(m.id);if(!request)return;pending.delete(m.id);m.error?request.reject(Error(JSON.stringify(m.error))):request.resolve(m.result);});
  ws.addEventListener('close',()=>{for(const request of pending.values())request.reject(Error('Owned WebView closed'));pending.clear();});
  await until(async()=>await evaluate("document.body?.innerText.includes('这是迁移验证版') ?? false"),'Vue mounted');
  check((await invoke('list_targets')).length>0,'Native window list empty');
  await click('识别通路');
  check(await evaluate("document.querySelector('h1').textContent.includes('验证原生识别')"),'Navigation failed');
  // Stub only the native file picker return, then use actual production buttons,
  // Vue state, Tauri command bridge, Rust native OCR and rendering.
  await evaluate(`(()=>{window.__labFiles=[];const original=window.fetch.bind(window);window.fetch=(url,options)=>{const command=decodeURIComponent(String(url));if(command.includes('ipc.localhost')&&(command.endsWith('plugin:dialog|open')||command.endsWith('plugin:dialog|save'))){if(!window.__labFiles.length)throw Error('No owned test file queued');return Promise.resolve(new Response(JSON.stringify(window.__labFiles.shift()),{headers:{'Content-Type':'application/json','Tauri-Response':'ok'}}))}return original(url,options)};return true})()`);
  await evaluate(`window.__labFiles.push(${JSON.stringify(path.join(repo,'image_for_test','03-游戏结束结算界面-主要用小队排名跟小队总击杀进行定位.jpg'))})`);
  await click('选择图片并验证');
  await until(async()=>await evaluate("[...document.querySelectorAll('.metrics strong')].some(e=>e.textContent.replace(/[^0-9]/g,'')==='158')"),'OCR result displayed');
  results.push('real UI button -> Rust Tesseract -> expected 158 damage');
  await click('旧数据读取');await evaluate(`window.__labFiles.push(${JSON.stringify(db)})`);await click('选择数据库');
  await until(async()=>await evaluate("document.querySelectorAll('tbody tr').length===3"),'legacy rows displayed');
  check(await evaluate("document.querySelector('table').innerText.includes('Archer')"),'Legacy nickname missing');
  check(Buffer.compare(await readFile(db),await readFile(path.join(repo,'data','matches.db')))===0,'Database bytes changed');
  results.push('real UI button -> Rust SQLite -> legacy rows, database bytes unchanged');
  await call('Page.enable');const screenshot=await call('Page.captureScreenshot',{format:'png'});await writeFile(path.join(output,'legacy-view.png'),Buffer.from(screenshot.data,'base64'));
  await click('画面采集');
  const own=(await invoke('list_targets')).find(t=>t.title==='Better Apex · Rust 兼容性验证');check(own,'Owned app window not found');
  await evaluate(`(()=>{const s=document.querySelector('select[aria-label="目标窗口"]');s.value=${JSON.stringify(String(own.handle))};s.dispatchEvent(new Event('change',{bubbles:true}));return true})()`);
  await click('开始采集验证');await until(async()=>(await invoke('capture_snapshot',{preview:false})).frames>0,'owned-window DXGI frame');
  await click('识别通路');check((await invoke('capture_snapshot',{preview:false})).running,'Navigation stopped capture');
  await click('画面采集');await click('停止采集验证');await until(async()=>!(await invoke('capture_snapshot',{preview:false})).running,'capture stopped');
  const png=path.join(output,'stopped-frame.png');await evaluate(`window.__labFiles.push(${JSON.stringify(png)})`);await click('保存已有画面的原始 PNG');
  await until(async()=>{try{return (await readFile(png)).length>100;}catch{return false;}},'save after stop');
  results.push('capture survives navigation; stopping retains one exportable frame without further acquisition');
  check((await(await fetch(`http://127.0.0.1:${port}/json/list`)).json()).filter(p=>p.type==='page'&&p.url.includes('tauri.localhost')).length===1,'More than one app WebView');
  results.push('one app WebView throughout navigation');
  await invoke('plugin:window|close',{label:'main'}).catch(()=>{});await until(()=>exited,'graceful app exit',10000);
  await writeFile(path.join(output,'report.json'),JSON.stringify({passed:true,results,logs},null,2));console.log(JSON.stringify({passed:true,results,output},null,2));
}catch(error){
  try{
    await writeFile(path.join(output,'failure.txt'),String(error)+'\n'+await evaluate('document.body?.innerText'));
    const shot=await call('Page.captureScreenshot',{format:'png'});await writeFile(path.join(output,'failure.png'),Buffer.from(shot.data,'base64'));
  }catch{}
  throw error;
}finally{
  if(!exited&&ws?.readyState===WebSocket.OPEN)await Promise.race([invoke('plugin:window|close',{label:'main'}).catch(()=>{}),delay(1000)]);
  if(ws?.readyState===WebSocket.OPEN)ws.close();
  if(!exited)child.kill();
  await writeFile(path.join(output,'runtime.log'),logs);await delay(200);
}
