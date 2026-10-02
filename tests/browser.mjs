import {createRequire} from 'node:module';
import {mkdtemp,realpath,rm,mkdir,readFile,writeFile,unlink} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import path from 'node:path';
import {tmpdir} from 'node:os';
import assert from 'node:assert/strict';
import {runDashboardRefreshTests} from './dashboard-refresh.mjs';
const playwrightPackage=process.env.SEREIN_PLAYWRIGHT_PACKAGE;
const require=createRequire(playwrightPackage?path.resolve(playwrightPackage):import.meta.url);
const {chromium}=require('playwright');
const root=process.cwd(), temp=await realpath(await mkdtemp(path.join(tmpdir(),'serein-browser-')));
const extension=path.join(root,'apps/extension/.output/chrome-mv3');
const browserRuntimeDir=process.env.SEREIN_BROWSER_RUNTIME_DIR?path.resolve(process.env.SEREIN_BROWSER_RUNTIME_DIR):null;
const evidence=process.env.SEREIN_BROWSER_EVIDENCE_DIR
 ? path.resolve(process.env.SEREIN_BROWSER_EVIDENCE_DIR)
 : path.join(root,'docs/screenshots');await mkdir(evidence,{recursive:true});
const resultsPath=process.env.SEREIN_BROWSER_RESULTS_PATH
 ? path.resolve(process.env.SEREIN_BROWSER_RESULTS_PATH)
 : path.join(root,'docs/browser-test-results.json');
await mkdir(path.dirname(resultsPath),{recursive:true});
let registeredPath;
let context;
let releaseMarker;
try{
 const launchOptions={headless:true,args:[`--disable-extensions-except=${extension}`,`--load-extension=${extension}`],viewport:{width:1320,height:940},env:{...process.env,SEREIN_DATA_DIR:path.join(temp,'data'),SEREIN_INSTALL_HOME:path.join(temp,'home')}};
 if(process.env.SEREIN_CHROMIUM_BIN)launchOptions.executablePath=process.env.SEREIN_CHROMIUM_BIN;
 context=await chromium.launchPersistentContext(temp,launchOptions);
 let worker=context.serviceWorkers()[0]||await context.waitForEvent('serviceworker');
 const id=new URL(worker.url()).host;
 const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.addInitScript(()=>{
  const probe={installed:false,enabled:false,holdPolicy:false,policyMessages:[],ticketReplies:[],releasePolicy:null};
  Object.defineProperty(window,'__sereinCopySetupProbe',{configurable:false,value:probe});
  const install=()=>{
   const runtimes=[globalThis.chrome?.runtime,globalThis.browser?.runtime].filter((runtime,index,all)=>runtime&&all.indexOf(runtime)===index);
   let installed=false;
   for(const runtime of runtimes){
    if(!runtime.sendMessage)continue;
    if(runtime.sendMessage.__sereinCopySetupProbe===probe||runtime.sendMessage.__sereinRefreshProbe){installed=true;continue}
    const original=runtime.sendMessage.bind(runtime);
    const wrapped=(message,...args)=>{
     if(probe.enabled&&message?.type==='policy'){
      probe.policyMessages.push({message,at:performance.now()});
      if(probe.holdPolicy){
       probe.holdPolicy=false;
       return new Promise((resolve,reject)=>{
        probe.releasePolicy=()=>{
         try{Promise.resolve(original(message,...args)).then(resolve,reject)}catch(error){reject(error)}
        };
       });
      }
     }
     const response=original(message,...args);
     if(!probe.enabled||message?.type!=='ticket')return response;
     return Promise.resolve(response).then(value=>{probe.ticketReplies.push(value);return value});
    };
    Object.defineProperty(wrapped,'__sereinCopySetupProbe',{value:probe});
    try{Object.defineProperty(runtime,'sendMessage',{configurable:true,writable:true,value:wrapped})}
    catch{runtime.sendMessage=wrapped}
    if(runtime.sendMessage===wrapped)installed=true;
   }
   probe.installed=installed;
   return installed;
  };
  install();
  const timer=setInterval(()=>{if(install())clearInterval(timer)},10);
  setTimeout(()=>clearInterval(timer),5000);
 });
 await page.goto(`chrome-extension://${id}/dashboard.html#connections`);await page.getByRole('heading',{name:'Connections',exact:true}).waitFor();
 await page.getByRole('link',{name:'https://github.com/Maar10Herr/serein',exact:true}).waitFor();
 await page.getByRole('button',{name:'Copy install request',exact:true}).waitFor();
 await page.screenshot({animations:'disabled',path:path.join(evidence,'onboarding-light.png'),fullPage:true});
 await page.waitForFunction(()=>window.__sereinCopySetupProbe?.installed,undefined,{timeout:10000});
 // Exercise the actual unpaired consent and setup-copy path in this disposable profile.
 const send=msg=>page.evaluate(msg=>chrome.runtime.sendMessage(msg),msg);
 const setTheme=async theme=>{
  for(let attempt=0;attempt<4;attempt++){
   await page.waitForTimeout(100);
   if(await page.evaluate(()=>document.documentElement.dataset.theme)===theme)return;
   await page.getByRole('button',{name:'Toggle color theme'}).click();
   await page.waitForTimeout(100);
  }
  assert.equal(await page.evaluate(()=>document.documentElement.dataset.theme),theme);
 };
 const state=await send({type:'state'});assert.equal(state.state.policy.consent,false);assert.equal(state.queued,0);
 const copySetup=page.getByRole('button',{name:'Copy link instruction',exact:true});
 assert.equal(await copySetup.isDisabled(),true,'setup copy must require both consent disclosures');
 await page.getByRole('checkbox',{name:'I agree to save permitted browsing metadata on this device.'}).check();
 await page.getByRole('checkbox',{name:'I allow assistant recall. Collection and indexing stay on this device. Context returned to an assistant may be sent to that assistant’s model provider.'}).check();
 const assistantButtons=page.locator('.assistant');
 let codexButton,genericButton;
 for(let index=0;index<await assistantButtons.count();index++){
  const button=assistantButtons.nth(index),label=await button.innerText();
  if(label.includes('Codex'))codexButton=button;
  if(label.includes('Generic local executor'))genericButton=button;
 }
 assert.ok(codexButton&&genericButton,'expected Codex and generic executor choices');
 assert.equal(await codexButton.getAttribute('aria-pressed'),'true');
 await codexButton.click();
 await genericButton.click();
 assert.equal(await genericButton.getAttribute('aria-pressed'),'true');
 assert.equal(await copySetup.isDisabled(),false);
 await page.evaluate(()=>{const probe=window.__sereinCopySetupProbe;probe.enabled=true;probe.holdPolicy=true});
 await page.evaluate(()=>{
  const button=[...document.querySelectorAll('button')].find(node=>node.textContent.trim()==='Copy link instruction');
  if(!button)throw new Error('copy setup button missing');
  button.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true}));
  button.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true}));
 });
 await page.waitForFunction(()=>window.__sereinCopySetupProbe?.releasePolicy,undefined,{timeout:10000});
 assert.equal(await copySetup.isDisabled(),true,'copy setup must lock while its consent policy mutation is pending');
 const pendingSetup=await page.evaluate(async()=>{
  const probe=window.__sereinCopySetupProbe,state=await chrome.runtime.sendMessage({type:'state'});
  return {policyCalls:probe.policyMessages.length,ticketReplies:probe.ticketReplies.length,consent:state.state.policy.consent,releaseReady:typeof probe.releasePolicy==='function'};
 });
 assert.deepEqual(pendingSetup,{policyCalls:1,ticketReplies:0,consent:false,releaseReady:true},'overlapping setup clicks must serialize before ticket creation');
 await page.evaluate(()=>window.__sereinCopySetupProbe.releasePolicy());
 await page.waitForFunction(()=>window.__sereinCopySetupProbe?.ticketReplies.length===1&&document.querySelector('textarea[readonly]')?.value,undefined,{timeout:15000});
 const setupInstructions=await page.locator('textarea[readonly]').inputValue();
 const ticketText=setupInstructions.slice(setupInstructions.lastIndexOf('\n\n')+2);
 const ticket=JSON.parse(ticketText);
 assert.equal(ticket.protocol,1);assert.equal(ticket.browser,'chrome');assert.equal(ticket.source_id,state.state.source_id);
 assert.equal(ticket.extension_id,id);assert.ok(ticket.nonce);assert.equal(ticket.consent,true);
 assert.equal(ticket.install_skills,false);assert.deepEqual(ticket.adapters,['generic']);
 assert.equal((await page.evaluate(()=>window.__sereinCopySetupProbe.policyMessages[0].message.patch)).consent,true);
 assert.equal((await page.evaluate(()=>window.__sereinCopySetupProbe.policyMessages[0].message.patch)).recall_enabled,true);
 assert.deepEqual(await page.evaluate(()=>window.__sereinCopySetupProbe.ticketReplies.map(reply=>reply.adapters)),[['generic']]);
 const setupState=await send({type:'state'});assert.equal(setupState.state.policy.consent,true);assert.equal(setupState.state.policy.recall_enabled,true);
 await send({type:'pause',paused:true});assert.equal((await send({type:'state'})).state.policy.paused,true);
 await page.reload();assert.equal((await send({type:'state'})).state.policy.paused,true);
 await send({type:'pause',paused:false});
 await send({type:'exclude',site:'privacy-check.example.net',forget:true});const after=await send({type:'state'});assert.ok(after.state.policy.excluded_sites.includes('privacy-check.example.net'));assert.ok(after.controls>=1);
 await page.getByRole('button',{name:'Your context',exact:true}).click();await page.getByRole('heading',{name:'Your context',exact:true}).waitFor();await setTheme('light');await page.screenshot({animations:'disabled',path:path.join(evidence,'context-empty-light.png'),fullPage:true});
 await setTheme('dark');await page.screenshot({animations:'disabled',path:path.join(evidence,'context-empty-dark.png'),fullPage:true});
 // Real runtime.sendNativeMessage round trip with a synthetic isolated vault.
 const nativeEnv={...process.env,SEREIN_DATA_DIR:path.join(temp,'data'),SEREIN_INSTALL_HOME:path.join(temp,'home')};
 const setup=JSON.parse(browserRuntimeDir
  ? execFileSync(path.join(browserRuntimeDir,'serein'),['setup','--request-stdin','--json'],{input:JSON.stringify(ticket),env:nativeEnv,cwd:browserRuntimeDir}).toString()
  : execFileSync('sh',[path.join(root,'skills/serein-context/scripts/connect.sh')],{input:JSON.stringify(ticket),env:nativeEnv}).toString());
 assert.equal(setup.status,'ok');
 const manifest=JSON.parse(await readFile(setup.manifest));
 const invocationLog=path.join(temp,'host-invocations.txt');
 const requestLog=path.join(temp,'native-host-requests.jsonl');
 releaseMarker=path.join(temp,'release-delayed-feedback');
 const wrapperConfigPath=path.join(temp,'native-host-wrapper-config.json');
 const wrapper=path.join(temp,'native-host-wrapper.mjs');
 await writeFile(wrapperConfigPath,JSON.stringify({delayAtomId:null,failureText:null}),{mode:0o600});
 const wrapperSource=`#!${process.execPath}
import fs from 'node:fs';
import {spawnSync} from 'node:child_process';
const config = ${JSON.stringify({nativeHost:manifest.path,invocationLog,requestLog,wrapperConfigPath,releaseMarker})};
const nativeArgs = process.argv.slice(2);
function readExact(size) {
  const bytes = Buffer.alloc(size);
  let offset = 0;
  while (offset < size) {
    const count = fs.readSync(0, bytes, offset, size - offset, null);
    if (count === 0) throw new Error('Incomplete native message from browser');
    offset += count;
  }
  return bytes;
}
function send(value) {
  const bytes = Buffer.from(JSON.stringify(value));
  const head = Buffer.alloc(4);
  head.writeUInt32LE(bytes.length);
  process.stdout.write(Buffer.concat([head, bytes]));
}
const head = readExact(4);
const length = head.readUInt32LE();
if (length < 1 || length > 262144) throw new Error('Invalid native message length');
const body = readExact(length);
const request = JSON.parse(body.toString('utf8'));
fs.appendFileSync(config.invocationLog, '1\\n', {mode: 0o600});
fs.appendFileSync(config.requestLog, JSON.stringify(request) + '\\n', {mode: 0o600});
const controls = JSON.parse(fs.readFileSync(config.wrapperConfigPath, 'utf8'));
if (request.op === 'feedback' && request.payload?.action === 'confirm_constraint') {
  if (request.payload.atom_id === controls.delayAtomId && request.payload.text?.startsWith('T07 delayed A ')) {
    const deadline = Date.now() + 20000;
    const pause = new Int32Array(new SharedArrayBuffer(4));
    while (!fs.existsSync(config.releaseMarker) && Date.now() < deadline)
      Atomics.wait(pause, 0, 0, 25);
    if (!fs.existsSync(config.releaseMarker)) {
      send({status:'error', error:{remedy:'Timed out waiting for the browser test release marker'}});
      process.exit(0);
    }
  }
  if (request.payload.text === controls.failureText) {
    send({status:'error', error:{remedy:'Synthetic feedback failure'}});
    process.exit(0);
  }
}
const result = spawnSync(config.nativeHost, nativeArgs, {input:Buffer.concat([head, body]), maxBuffer:524288});
if (result.error) throw result.error;
if (result.stdout) process.stdout.write(result.stdout);
process.exit(result.status ?? 1);
`;
 await writeFile(wrapper,wrapperSource,{mode:0o700});
 manifest.path=wrapper;
 const nativeCalls=async()=>((await readFile(invocationLog,'utf8').catch(()=>''))).split('\n').filter(Boolean).length;
 const nativeDirectory=path.join(temp,'NativeMessagingHosts');
 await mkdir(nativeDirectory,{recursive:true});
 const candidatePath=path.join(nativeDirectory,'com.serein.context.json');
 await writeFile(candidatePath,JSON.stringify(manifest),{flag:'wx',mode:0o600});registeredPath=candidatePath;
 await page.waitForFunction(async ()=>(await chrome.runtime.sendMessage({type:'state'})).state.paired,{},{timeout:15000});
 const connected=await send({type:'state'});assert.equal(connected.state.paired,true);assert.equal(connected.controls,0,JSON.stringify(connected));
 await page.goto(`chrome-extension://${id}/dashboard.html#connections`);
 await page.reload();
 await page.getByRole('heading',{name:"You're connected",exact:true}).waitFor();
 const firstRunQuestion=page.locator('.first-run-question');
 await firstRunQuestion.waitFor();
 assert.equal(await firstRunQuestion.innerText(),'“What did I find about [the topic I researched]?”');
 await page.screenshot({animations:'disabled',path:path.join(evidence,'connected-first-run.png'),fullPage:true});
 const baseline=await nativeCalls();
 const queueSynthetic=async(count,ageMs=0)=>page.evaluate(async ({count,ageMs})=>{
   const request=indexedDB.open('serein',1);
   const database=await new Promise((resolve,reject)=>{request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error)});
   const tx=database.transaction(['outbox','state'],'readwrite');
   const stateRequest=tx.objectStore('state').get('config');
   const current=await new Promise((resolve,reject)=>{stateRequest.onsuccess=()=>resolve(stateRequest.result);stateRequest.onerror=()=>reject(stateRequest.error)});
   current.batchSince=Date.now()-ageMs;tx.objectStore('state').put(current,'config');
   for(let i=0;i<count;i++)tx.objectStore('outbox').put({event_id:crypto.randomUUID(),visit_id:crypto.randomUUID(),site_key:'batch.example.com',site_epoch:0,observed_at:new Date().toISOString(),kind:'search',title:'Synthetic batch event',search_query:'batch experiment',foreground_seconds:30});
   await new Promise((resolve,reject)=>{tx.oncomplete=resolve;tx.onerror=()=>reject(tx.error)});database.close();
 },{count,ageMs});
 await queueSynthetic(19);
 assert.equal((await send({type:'state'})).queued,19);
 assert.equal(await nativeCalls(),baseline,'under-threshold observations launched the helper');
 await queueSynthetic(1);
 assert.equal((await send({type:'state'})).queued,0);
 assert.equal(await nativeCalls(),baseline+1,'twenty observations should launch one helper');
 await queueSynthetic(1,61_000);
 assert.equal((await send({type:'state'})).queued,0);
 assert.equal(await nativeCalls(),baseline+2,'a one-minute-old observation should flush');
 // Exercise the real browser alarm while no extension document is open.
 await queueSynthetic(1);
 assert.equal((await send({type:'state'})).queued,1);
 const alarmBaseline=await nativeCalls();
 await page.goto('about:blank');
 const alarmDeadline=Date.now()+95_000;
 while(await nativeCalls()===alarmBaseline && Date.now()<alarmDeadline)
   await new Promise(resolve=>setTimeout(resolve,1000));
 assert.equal(await nativeCalls(),alarmBaseline+1,'browser alarm did not launch exactly one helper');
 await page.goto(`chrome-extension://${id}/dashboard.html#context`);
 assert.equal((await send({type:'state'})).queued,0,'alarm launch did not drain the outbox');
 const sample={event_id:crypto.randomUUID(),visit_id:crypto.randomUUID(),site_key:'example.com',site_epoch:0,observed_at:new Date().toISOString(),kind:'search',title:'Search for a desk lamp',search_query:'Desk lamp research',foreground_seconds:30};
 const nativeRequest={protocol:1,request_id:crypto.randomUUID(),source_id:ticket.source_id,op:'ingest',capture_epoch:connected.state.policy.capture_epoch,payload:{events:[sample]}};
 const acknowledgement=await page.evaluate(request=>chrome.runtime.sendNativeMessage('com.serein.context',request),nativeRequest);assert.deepEqual(acknowledgement.acknowledged_ids,[sample.event_id]);
 const second=await page.evaluate(request=>chrome.runtime.sendNativeMessage('com.serein.context',request),nativeRequest);assert.deepEqual(second.duplicate_ids,[sample.event_id]);
 const textA='T07 observed card A';
 const textB='T07 observed card B';
 const correctionEvents=[
  {event_id:crypto.randomUUID(),visit_id:crypto.randomUUID(),site_key:'t07-a.example.org',site_epoch:0,observed_at:new Date().toISOString(),kind:'search',title:textA,search_query:textA,foreground_seconds:30},
  {event_id:crypto.randomUUID(),visit_id:crypto.randomUUID(),site_key:'t07-b.example.org',site_epoch:0,observed_at:new Date().toISOString(),kind:'search',title:textB,search_query:textB,foreground_seconds:30},
 ];
 const correctionIngest={protocol:1,request_id:crypto.randomUUID(),source_id:ticket.source_id,op:'ingest',capture_epoch:connected.state.policy.capture_epoch,payload:{events:correctionEvents}};
 const correctionAck=await page.evaluate(request=>chrome.runtime.sendNativeMessage('com.serein.context',request),correctionIngest);
 assert.deepEqual(correctionAck.acknowledged_ids,correctionEvents.map(event=>event.event_id));
 const correctionDashboard=await send({type:'host',op:'dashboard'});
 const cardA=correctionDashboard.cards.find(card=>card.text===textA);
 const cardB=correctionDashboard.cards.find(card=>card.text===textB);
 assert.ok(cardA&&cardB,`correction fixtures missing from dashboard: ${JSON.stringify(correctionDashboard.cards.map(card=>({id:card.id,text:card.text,state:card.state})))}`);
 await page.reload();
 await page.getByRole('heading',{name:'Research memories',exact:true}).waitFor();
 await page.getByRole('heading',{name:'Recent useful evidence',exact:true}).waitFor();
 const rawActivity=page.locator('.raw-activity details');await rawActivity.waitFor();
 assert.equal(await rawActivity.evaluate(node=>node.open),false,'raw activity should start collapsed');
 await rawActivity.locator('summary').click();
 await rawActivity.getByRole('heading',{name:'Desk lamp research',exact:true}).waitFor();
 const openCorrection=async text=>{
  const card=page.locator('.raw-activity .evidence-card').filter({hasText:text});
  assert.equal(await card.count(),1,`expected one rendered card for ${text}`);
  await card.getByRole('button',{name:'Correct',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Correct this context',exact:true});
  await dialog.waitFor();
  return dialog;
 };
 const cancelCorrection=async (dialog,method)=>{
  if(method==='Escape')await page.keyboard.press('Escape');
  else if(method==='Close')await dialog.getByRole('button',{name:'Close',exact:true}).click();
  else if(method==='backdrop')await page.locator('.modal-backdrop').click({position:{x:2,y:2}});
  else throw new Error(`unknown correction cancellation method: ${method}`);
  await dialog.waitFor({state:'detached'});
 };
 const inspectModalFocus=dialog=>dialog.evaluate(node=>{
  const selector='a[href],area[href],button,input,select,textarea,iframe,object,embed,summary,audio[controls],video[controls],[contenteditable="true"],[tabindex]';
  const usable=element=>{
   if(!element.isConnected||element.matches(':disabled')||element.tabIndex<0)return false;
   if(element instanceof HTMLInputElement&&element.type==='hidden')return false;
   for(let ancestor=element;ancestor;ancestor=ancestor.parentElement){
    if(ancestor.hidden||ancestor.inert||ancestor.getAttribute('aria-hidden')?.toLowerCase()==='true')return false;
    const style=getComputedStyle(ancestor);
    if(style.display==='none'||style.visibility==='hidden'||style.visibility==='collapse'||style.getPropertyValue('content-visibility')==='hidden')return false;
    if(ancestor===node)return element.getClientRects().length>0;
   }
   return false;
  };
  const candidates=[...node.querySelectorAll(selector)].map((element,domOrder)=>({element,domOrder})).filter(({element})=>usable(element)).sort((a,b)=>{
   const aIndex=a.element.tabIndex,bIndex=b.element.tabIndex;
   if(aIndex>0&&bIndex>0)return aIndex-bIndex||a.domOrder-b.domOrder;
   if(aIndex>0)return -1;
   if(bIndex>0)return 1;
   return a.domOrder-b.domOrder;
  }).map(({element})=>element);
  const active=document.activeElement;
  return {inside:node.contains(active),activeIndex:candidates.indexOf(active),activeIsDialog:active===node,activeTag:active?.tagName??null,activeId:active?.id??'',activeLabel:active?.getAttribute?.('aria-label')||active?.textContent?.trim()||'',dialogTabIndex:node.tabIndex,candidates:candidates.map(element=>({id:element.id,label:element.getAttribute('aria-label')||element.textContent?.trim()||element.tagName,tag:element.tagName,tabIndex:element.tabIndex}))};
 });
 const inspectModalSetup=()=>page.evaluate(()=>{
  const dialog=document.querySelector('[role="dialog"]');
  const active=document.activeElement;
  return {at:performance.now(),activeTag:active?.tagName??null,activeId:active?.id??'',activeLabel:active?.getAttribute?.('aria-label')||active?.textContent?.trim()||'',dialogPresent:!!dialog,dialogContainsActive:!!dialog?.contains(active),mainInert:document.querySelector('main')?.inert??false,dialogInert:!!dialog?.closest('[inert]')};
 });
 const waitForModalSetup=async()=>{
  const initial=await inspectModalSetup();let timedOut=false;
  try{
   await page.waitForFunction(()=>{
    const dialog=document.querySelector('[role="dialog"]');
    return !!dialog&&dialog.contains(document.activeElement)&&document.querySelector('main')?.inert===true&&!dialog.closest('[inert]');
   },undefined,{timeout:1000});
  }catch{timedOut=true}
  const settled=await inspectModalSetup();
  return {initial,settled,delayMs:settled.at-initial.at,timedOut};
 };
 const nativeRequests=async()=>{
  const text=await readFile(requestLog,'utf8').catch(()=> '');
  return text.trim()?text.trim().split('\n').map(line=>JSON.parse(line)):[];
 };
 const waitForRequest=async predicate=>{
  const deadline=Date.now()+15000;
  while(Date.now()<deadline){
   const request=(await nativeRequests()).find(predicate);
   if(request)return request;
   await new Promise(resolve=>setTimeout(resolve,50));
  }
  assert.fail('timed out waiting for the expected native feedback request');
 };
 // R18: cancellation must destroy the card-specific draft for every supported path.
 for(const method of ['Escape','Close','backdrop']){
  const draft=`T07 canceled A draft ${method}`;
  const dialogA=await openCorrection(textA);
  await dialogA.locator('textarea').fill(draft);
  assert.equal(await dialogA.locator('textarea').inputValue(),draft);
  await cancelCorrection(dialogA,method);
  const dialogB=await openCorrection(textB);
  assert.equal(await dialogB.locator('textarea').inputValue(),'','draft leaked from canceled A to B via '+method);
  await cancelCorrection(dialogB,'Close');
 }
 const r18Text='T07 B confirmation text only';
 const dialogB=await openCorrection(textB);
 assert.equal(await dialogB.locator('textarea').inputValue(),'');
 await dialogB.locator('textarea').fill(r18Text);
 await dialogB.getByRole('button',{name:'Confirm constraint',exact:true}).click();
 await dialogB.waitFor({state:'detached'});
 let loggedFeedback=(await nativeRequests()).filter(request=>request.op==='feedback');
 assert.equal(loggedFeedback.length,1,'canceling a draft must not submit a feedback request');
 assert.deepEqual(
  {atom_id:loggedFeedback[0].payload.atom_id,action:loggedFeedback[0].payload.action,text:loggedFeedback[0].payload.text},
  {atom_id:cardB.id,action:'confirm_constraint',text:r18Text},
  'R18 must send only B’s atom ID and text'
 );
 const r18Dashboard=await send({type:'host',op:'dashboard'});
 assert.equal(r18Dashboard.cards.find(card=>card.id===cardB.id)?.text,r18Text);
 assert.equal(r18Dashboard.cards.find(card=>card.id===cardA.id)?.text,textA);
 await page.screenshot({animations:'disabled',path:path.join(evidence,'correction-confirmed-b.png'),fullPage:true});
 // R19: resolve delayed requests after switching cards and after reopening the same card.
 await writeFile(wrapperConfigPath,JSON.stringify({delayAtomId:cardA.id,failureText:null}));
 const delayedTextA1='T07 delayed A first';
 let delayedA=await openCorrection(textA);
 await delayedA.locator('textarea').fill(delayedTextA1);
 await delayedA.getByRole('button',{name:'Confirm constraint',exact:true}).click();
 await waitForRequest(request=>request.op==='feedback'&&request.payload?.atom_id===cardA.id&&request.payload?.text===delayedTextA1);
 await cancelCorrection(delayedA,'Escape');
 let pendingB=await openCorrection(r18Text);
 const pendingBDraft='T07 B remains open during A response';
 await pendingB.locator('textarea').fill(pendingBDraft);
 await writeFile(releaseMarker,'release one delayed A response');
 await page.waitForFunction(async ({atomId,text})=>{
  const response=await chrome.runtime.sendMessage({type:'host',op:'dashboard'});
  return response.cards.some(card=>card.id===atomId&&card.text===text);
 },{atomId:cardA.id,text:delayedTextA1},{timeout:15000});
 assert.equal(await page.getByRole('dialog',{name:'Correct this context',exact:true}).count(),1,'old A response closed B’s dialog');
 assert.equal(await pendingB.locator('textarea').inputValue(),pendingBDraft,'old A response cleared B’s draft');
 await cancelCorrection(pendingB,'Escape');
 await unlink(releaseMarker);
 await page.reload();
 await page.getByRole('heading',{name:'Research memories',exact:true}).waitFor();
 await rawActivity.waitFor();
 if(!(await rawActivity.evaluate(node=>node.open)))await rawActivity.locator('summary').click();
 await page.locator('.raw-activity .evidence-card').filter({hasText:delayedTextA1}).waitFor();
 const delayedTextA2='T07 delayed A second';
 delayedA=await openCorrection(delayedTextA1);
 await delayedA.locator('textarea').fill(delayedTextA2);
 await delayedA.getByRole('button',{name:'Confirm constraint',exact:true}).click();
 await waitForRequest(request=>request.op==='feedback'&&request.payload?.atom_id===cardA.id&&request.payload?.text===delayedTextA2);
 await cancelCorrection(delayedA,'Escape');
 let reopenedA=await openCorrection(delayedTextA1);
 const reopenedDraft='T07 reopened A draft stays';
 assert.equal(await reopenedA.locator('textarea').inputValue(),'');
 await reopenedA.locator('textarea').fill(reopenedDraft);
 await writeFile(releaseMarker,'release second delayed A response');
 await page.waitForFunction(async ({atomId,text})=>{
  const response=await chrome.runtime.sendMessage({type:'host',op:'dashboard'});
  return response.cards.some(card=>card.id===atomId&&card.text===text);
 },{atomId:cardA.id,text:delayedTextA2},{timeout:15000});
 assert.equal(await page.getByRole('dialog',{name:'Correct this context',exact:true}).count(),1,'old response closed a newer instance of A’s dialog');
 assert.equal(await reopenedA.locator('textarea').inputValue(),reopenedDraft,'old response cleared a newer instance of A’s draft');
 await cancelCorrection(reopenedA,'Close');
 // A failure in the still-current instance must leave its text available for retry.
 const failureText='T07 current instance failure draft';
 await writeFile(wrapperConfigPath,JSON.stringify({delayAtomId:cardA.id,failureText}));
 const failedDialog=await openCorrection(r18Text);
 await failedDialog.locator('textarea').fill(failureText);
 await failedDialog.getByRole('button',{name:'Confirm constraint',exact:true}).click();
 await failedDialog.getByRole('alert').filter({hasText:'Synthetic feedback failure'}).waitFor();
 assert.equal(await failedDialog.locator('textarea').inputValue(),failureText,'failed confirmation discarded the current draft');
 assert.equal(await failedDialog.count(),1,'failed confirmation closed the current dialog');
 await cancelCorrection(failedDialog,'Close');
 loggedFeedback=(await nativeRequests()).filter(request=>request.op==='feedback');
 assert.deepEqual(loggedFeedback.map(request=>({atom_id:request.payload.atom_id,action:request.payload.action,text:request.payload.text})),[
  {atom_id:cardB.id,action:'confirm_constraint',text:r18Text},
  {atom_id:cardA.id,action:'confirm_constraint',text:delayedTextA1},
  {atom_id:cardA.id,action:'confirm_constraint',text:delayedTextA2},
  {atom_id:cardB.id,action:'confirm_constraint',text:failureText},
 ],'only the deliberate confirmations reached the native helper');
 // R20: real dashboard Modal containment, live candidates and safe restoration.
 const r20Card=page.locator('.raw-activity .evidence-card').filter({hasText:r18Text});
 assert.equal(await r20Card.count(),1,'R20 target card must remain visible');
 await r20Card.getByRole('button',{name:'Correct',exact:true}).evaluate(button=>{button.id='r20-trigger'});
 let r20Dialog=await openCorrection(r18Text);
 const r20Setup=await waitForModalSetup();
 await writeFile(path.join(path.dirname(resultsPath),'r20-modal-initial-setup.json'),JSON.stringify({browser:'Chrome',setup:r20Setup},null,2)+'\n');
 assert.equal(r20Setup.settled.dialogContainsActive,true,`initial focus must enter the dialog after setup: ${JSON.stringify(r20Setup)}`);
 assert.equal(r20Setup.settled.mainInert,true,`the modal setup must inert the background: ${JSON.stringify(r20Setup)}`);
 assert.equal(r20Setup.settled.dialogInert,false,`the modal must remain outside inert ancestors: ${JSON.stringify(r20Setup)}`);
 let r20Focus=await inspectModalFocus(r20Dialog);
 assert.equal(await r20Dialog.evaluate(dialog=>dialog.tabIndex),-1,'the dialog container must support the zero-tabbable fallback');
 assert.equal(r20Focus.inside,true,`initial focus must be contained by the dialog: ${JSON.stringify({setup:r20Setup,focus:r20Focus})}`);
 assert.equal(r20Focus.activeIndex,0,'opening the dialog should focus its first enabled control');
 assert.equal(r20Focus.activeLabel,'Close');
 assert.equal(await r20Dialog.getByRole('button',{name:'Confirm constraint',exact:true}).isDisabled(),true,'empty confirmation should be disabled');
 assert.equal(r20Focus.candidates.some(candidate=>candidate.label==='Confirm constraint'),false,'a disabled confirmation must not be tabbable');
 const openInertState=await page.evaluate(()=>({mainInert:document.querySelector('main')?.inert??false,dialogInert:!!document.querySelector('[role="dialog"]')?.closest('[inert]')}));
 assert.equal(openInertState.mainInert,true,'background main content must be inert while a dialog is open');
 assert.equal(openInertState.dialogInert,false,'the dialog must not inherit inertness from its own ancestor');
 await r20Dialog.evaluate(dialog=>{
  const positiveFirst=document.createElement('button');positiveFirst.id='r20-positive-first';positiveFirst.tabIndex=2;positiveFirst.textContent='Positive first';
  const positiveSecond=document.createElement('button');positiveSecond.id='r20-positive-second';positiveSecond.tabIndex=2;positiveSecond.textContent='Positive second';
  dialog.append(positiveFirst,positiveSecond);
  const fieldset=document.createElement('fieldset');fieldset.disabled=true;const disabled=document.createElement('button');disabled.id='r20-disabled-fieldset';disabled.textContent='Disabled fieldset control';fieldset.append(disabled);dialog.append(fieldset);
  const hidden=document.createElement('button');hidden.id='r20-hidden-control';hidden.hidden=true;hidden.textContent='Hidden control';dialog.append(hidden);
  const inertParent=document.createElement('div');inertParent.inert=true;const inert=document.createElement('button');inert.id='r20-inert-control';inert.textContent='Inert control';inertParent.append(inert);dialog.append(inertParent);
 });
 r20Focus=await inspectModalFocus(r20Dialog);
 assert.deepEqual(r20Focus.candidates.slice(0,2).map(candidate=>candidate.id),['r20-positive-first','r20-positive-second'],'equal positive tabindex values must retain DOM order');
 assert.ok(!r20Focus.candidates.some(candidate=>['r20-disabled-fieldset','r20-hidden-control','r20-inert-control'].includes(candidate.id)),'disabled fieldset, hidden, and inert descendants must not be tabbable');
 await page.evaluate(()=>{document.body.tabIndex=-1;document.body.focus()});
 await page.keyboard.press('Tab');r20Focus=await inspectModalFocus(r20Dialog);
 assert.equal(r20Focus.inside,true,'Tab from outside must enter the dialog');assert.equal(r20Focus.activeIndex,0);assert.equal(r20Focus.activeId,'r20-positive-first');
 for(let expected=1;expected<r20Focus.candidates.length;expected++){
  await page.keyboard.press('Tab');r20Focus=await inspectModalFocus(r20Dialog);
  assert.equal(r20Focus.inside,true,`forward Tab escaped the dialog at candidate ${expected}`);assert.equal(r20Focus.activeIndex,expected,`forward Tab skipped candidate ${expected}`);
 }
 await page.keyboard.press('Tab');r20Focus=await inspectModalFocus(r20Dialog);assert.equal(r20Focus.activeIndex,0,'Tab from the final candidate must wrap to the first');
 for(let expected=r20Focus.candidates.length-1;expected>=0;expected--){
  await page.keyboard.press('Shift+Tab');r20Focus=await inspectModalFocus(r20Dialog);
  assert.equal(r20Focus.inside,true,`reverse Tab escaped the dialog at candidate ${expected}`);assert.equal(r20Focus.activeIndex,expected,`reverse Tab skipped candidate ${expected}`);
 }
 await page.evaluate(()=>{document.body.focus()});await page.keyboard.press('Shift+Tab');r20Focus=await inspectModalFocus(r20Dialog);
 assert.equal(r20Focus.inside,true,'Shift+Tab from outside must enter the dialog');assert.equal(r20Focus.activeIndex,r20Focus.candidates.length-1);
 await r20Dialog.locator('textarea').fill('T08 enabled traversal draft');
 assert.equal(await r20Dialog.getByRole('button',{name:'Confirm constraint',exact:true}).isEnabled(),true,'nonempty confirmation should be enabled');
 r20Focus=await inspectModalFocus(r20Dialog);assert.equal(r20Focus.candidates.at(-1)?.label,'Confirm constraint','the enabled submit control should join the live tabbable set');
 await page.keyboard.press('Tab');r20Focus=await inspectModalFocus(r20Dialog);assert.equal(r20Focus.activeLabel,'Confirm constraint');
 await page.keyboard.press('Shift+Tab');r20Focus=await inspectModalFocus(r20Dialog);assert.equal(r20Focus.activeTag,'TEXTAREA');
 await r20Dialog.getByRole('button',{name:'Close',exact:true}).click();await r20Dialog.waitFor({state:'detached'});
 let restoredFocus=await page.evaluate(()=>({id:document.activeElement?.id??'',connected:document.activeElement?.isConnected??false,mainInert:document.querySelector('main')?.inert??false}));
 assert.equal(restoredFocus.id,'r20-trigger','Close should restore focus to the still-usable trigger');assert.equal(restoredFocus.connected,true);assert.equal(restoredFocus.mainInert,false,'background inert state must be restored on close');
 r20Dialog=await openCorrection(r18Text);
 const noTabSetup=await waitForModalSetup();
 await writeFile(path.join(path.dirname(resultsPath),'r20-modal-no-tabbable-setup.json'),JSON.stringify({browser:'Chrome',setup:noTabSetup},null,2)+'\n');
 assert.equal(noTabSetup.settled.dialogContainsActive,true,`zero-tabbable modal setup must focus the dialog: ${JSON.stringify(noTabSetup)}`);
 assert.equal(noTabSetup.settled.mainInert,true,`zero-tabbable modal setup must inert the background: ${JSON.stringify(noTabSetup)}`);
 assert.equal(noTabSetup.settled.dialogInert,false,`zero-tabbable modal must remain outside inert ancestors: ${JSON.stringify(noTabSetup)}`);
 await r20Dialog.evaluate(dialog=>{
  for(const element of dialog.querySelectorAll('a[href],area[href],button,input,select,textarea,iframe,object,embed,summary,audio[controls],video[controls],[contenteditable="true"],[tabindex]')){
   element.hidden=true;element.tabIndex=-1;if('disabled' in element)element.disabled=true;
  }
 });
 const noTabBefore=await page.evaluate(()=>{
  const dialog=document.querySelector('[role="dialog"]');const active=document.activeElement;
  window.__r20NoTabKeys=[];
  document.addEventListener('keydown',event=>window.__r20NoTabKeys.push({key:event.key,activeTag:document.activeElement?.tagName??null,activeId:document.activeElement?.id??'',defaultPrevented:event.defaultPrevented}),true);
  return {activeTag:active?.tagName??null,activeId:active?.id??'',dialogContainsActive:!!dialog?.contains(active),mainInert:document.querySelector('main')?.inert??false,dialogInert:!!dialog?.closest('[inert]')};
 });
 await page.evaluate(()=>document.body.focus());
 const noTabAfterBodyFocus=await page.evaluate(()=>({activeTag:document.activeElement?.tagName??null,activeId:document.activeElement?.id??''}));
 await page.keyboard.press('Tab');r20Focus=await inspectModalFocus(r20Dialog);
 const noTabDiagnostics=await page.evaluate(()=>({keys:window.__r20NoTabKeys,activeTag:document.activeElement?.tagName??null,activeId:document.activeElement?.id??'',mainInert:document.querySelector('main')?.inert??false,dialogInert:!!document.querySelector('[role="dialog"]')?.closest('[inert]')}));
 const noTabFocus={...r20Focus};
 await writeFile(path.join(path.dirname(resultsPath),'r20-modal-no-tabbables.json'),JSON.stringify({browser:'Chrome',noTabBefore,noTabAfterBodyFocus,noTabDiagnostics,focus:r20Focus},null,2)+'\n');
 assert.equal(r20Focus.inside,true,`Tab with no tabbables must focus the dialog container: ${JSON.stringify({noTabBefore,noTabAfterBodyFocus,noTabDiagnostics,focus:r20Focus})}`);assert.equal(r20Focus.activeIsDialog,true,`zero-tabbable fallback did not focus the dialog: ${JSON.stringify({noTabBefore,noTabAfterBodyFocus,noTabDiagnostics,focus:r20Focus})}`);assert.equal(r20Focus.activeIndex,-1);
 await page.locator('#r20-trigger').evaluate(trigger=>{window.__r20RemovedTrigger=trigger;trigger.remove()});
 assert.equal(await page.evaluate(()=>window.__r20RemovedTrigger.isConnected),false,'removed-trigger fixture was not detached');
 await cancelCorrection(r20Dialog,'Escape');
 const fallbackFocus=await page.evaluate(()=>({insideMain:!!document.querySelector('main')?.contains(document.activeElement),tag:document.activeElement?.tagName??'',body:document.activeElement===document.body,mainInert:document.querySelector('main')?.inert??false}));
 assert.equal(fallbackFocus.insideMain,true,'Escape after trigger removal should focus the stable main heading/container');assert.equal(fallbackFocus.body,false);assert.equal(fallbackFocus.mainInert,false);
 await page.reload();await page.getByRole('heading',{name:'Research memories',exact:true}).waitFor();await rawActivity.waitFor();if(!(await rawActivity.evaluate(node=>node.open)))await rawActivity.locator('summary').click();
 await unlink(releaseMarker).catch(()=>{});await writeFile(wrapperConfigPath,JSON.stringify({delayAtomId:cardB.id,failureText:null}));
 const r20SubmitText='T07 delayed A R20 submitting';
 r20Dialog=await openCorrection(r18Text);const submittingSetup=await waitForModalSetup();await r20Dialog.locator('textarea').fill(r20SubmitText);await r20Dialog.getByRole('button',{name:'Confirm constraint',exact:true}).click();
 await waitForRequest(request=>request.op==='feedback'&&request.payload?.atom_id===cardB.id&&request.payload?.text===r20SubmitText);
 assert.equal(await r20Dialog.count(),1,'the delayed submit should keep its dialog mounted');
 const submittingButtons=await r20Dialog.locator('button').evaluateAll(buttons=>buttons.map(button=>({label:button.getAttribute('aria-label')||button.textContent.trim(),disabled:button.disabled})));
 assert.equal(submittingButtons.find(button=>button.label==='Confirm constraint')?.disabled,true,'the confirmation control must be disabled while submitting');
 assert.ok(submittingButtons.filter(button=>button.label!=='Close').every(button=>button.disabled),'all correction actions must be disabled while submitting');
 assert.equal(submittingButtons.find(button=>button.label==='Close')?.disabled,false,'Close should remain available while submitting');
 r20Focus=await inspectModalFocus(r20Dialog);assert.equal(r20Focus.candidates.length,2,'submitting state should recompute to Close and the still-enabled textarea');
 await page.keyboard.press('Tab');r20Focus=await inspectModalFocus(r20Dialog);assert.equal(r20Focus.inside,true);assert.equal(r20Focus.activeIndex,0,'Tab after disabling the former last action should return to the first live candidate');
 await page.keyboard.press('Shift+Tab');r20Focus=await inspectModalFocus(r20Dialog);assert.equal(r20Focus.inside,true);assert.equal(r20Focus.activeTag,'TEXTAREA','reverse traversal should use the recomputed last live candidate');
 await writeFile(releaseMarker,'release R20 delayed confirmation');
 await page.waitForFunction(async ({atomId,text})=>(await chrome.runtime.sendMessage({type:'host',op:'dashboard'})).cards.some(card=>card.id===atomId&&card.text===text),{atomId:cardB.id,text:r20SubmitText},{timeout:15000});
 await r20Dialog.waitFor({state:'detached'});
 loggedFeedback=(await nativeRequests()).filter(request=>request.op==='feedback');
 assert.deepEqual(loggedFeedback.map(request=>({atom_id:request.payload.atom_id,action:request.payload.action,text:request.payload.text})),[
  {atom_id:cardB.id,action:'confirm_constraint',text:r18Text},
  {atom_id:cardA.id,action:'confirm_constraint',text:delayedTextA1},
  {atom_id:cardA.id,action:'confirm_constraint',text:delayedTextA2},
  {atom_id:cardB.id,action:'confirm_constraint',text:failureText},
  {atom_id:cardB.id,action:'confirm_constraint',text:r20SubmitText},
 ],'only the four R18/R19 confirmations and the delayed R20 submitting fixture should reach native messaging');
 if(process.env.SEREIN_BROWSER_R20_RESULTS_PATH){
  const r20ResultsPath=path.resolve(process.env.SEREIN_BROWSER_R20_RESULTS_PATH);
  await mkdir(path.dirname(r20ResultsPath),{recursive:true});
  await writeFile(r20ResultsPath,JSON.stringify({status:'PASS',scope:'R18–R20 actual Chrome browser checks completed before the dashboard refresh suite',browser:await context.browser()?.version(),extension_id:id,nativeRuntime:browserRuntimeDir||'skills/serein-context bundled helper via connect.sh and recall.sh',modalInitialSetup:{first:r20Setup,zeroTab:noTabSetup,submitting:submittingSetup},zeroTabDiagnostics:{before:noTabBefore,afterBodyFocus:noTabAfterBodyFocus,afterTab:noTabDiagnostics,focus:noTabFocus},feedbackRequests:loggedFeedback.map(request=>({atom_id:request.payload.atom_id,action:request.payload.action,text:request.payload.text})),fullLaneResultsPath:resultsPath},null,2)+'\n');
 }
 await page.goto(`chrome-extension://${id}/dashboard.html#connections`);
 assert.equal(await page.getByRole('heading',{name:"You're connected",exact:true}).count(),0,'first-run prompt should end after the first saved observation');
 await page.goto(`chrome-extension://${id}/dashboard.html#context`);
 await setTheme('dark');await page.screenshot({animations:'disabled',path:path.join(evidence,'context-populated-dark.png'),fullPage:true});
 await setTheme('light');await page.screenshot({animations:'disabled',path:path.join(evidence,'context-populated-light.png'),fullPage:true});
 const recallRequest={protocol:1,request_id:crypto.randomUUID(),client:'generic',vault:'default',query:'desk lamp research',facets:['desk'],scope:['research'],max_bytes:4096,budget_ms:1500};
 const recallOutput=browserRuntimeDir
  ? execFileSync(path.join(browserRuntimeDir,'serein'),['recall','--request-stdin','--json'],{input:JSON.stringify(recallRequest),env:nativeEnv,cwd:browserRuntimeDir}).toString()
  : execFileSync('sh',[path.join(root,'skills/serein-context/scripts/recall.sh')],{input:JSON.stringify(recallRequest),env:nativeEnv}).toString();
 const recalled=JSON.parse(recallOutput);assert.equal(recalled.context.length,1);
 await send({type:'exclude',site:'example.com',forget:true});const forgotten=await send({type:'host',op:'dashboard'});assert.ok(!forgotten.cards.some(card=>card.text==='Desk lamp research'),'the original synthetic observation was not forgotten');
 await send({type:'exclude',site:'t07-a.example.org',forget:true});
 await send({type:'exclude',site:'t07-b.example.org',forget:true});
 assert.equal((await send({type:'host',op:'dashboard'})).cards.length,0,'T07 synthetic records were not forgotten from the disposable vault');
 await page.evaluate(()=>chrome.storage.local.set({theme:'dark'}));await page.goto(`chrome-extension://${id}/popup.html`);await page.getByRole('button',{name:'Settings',exact:true}).waitFor();await page.waitForFunction(()=>document.documentElement.dataset.theme==='dark');await page.setViewportSize({width:360,height:380});await page.screenshot({animations:'disabled',path:path.join(evidence,'popup-dark.png')});
 await page.keyboard.press('Tab');assert.ok(await page.evaluate(()=>document.activeElement?.tagName==='BUTTON'));
 await page.goto(`chrome-extension://${id}/dashboard.html#context`);
 for(const [locale,heading] of [['zh-CN','你的上下文'],['ja','あなたのコンテキスト'],['es','Tu contexto'],['de','Dein Kontext'],['nl','Jouw context'],['en','Your context']]){
  await page.locator('select').first().selectOption(locale);
  await page.getByRole('heading',{name:heading,exact:true}).waitFor();
  await page.waitForFunction(expected=>document.documentElement.lang===expected,locale);
  assert.equal(await page.locator('html').getAttribute('lang'),locale);
  await page.waitForFunction(async expected=>(await chrome.storage.local.get('locale')).locale===expected,locale);
  await page.reload();
  await page.getByRole('heading',{name:heading,exact:true}).waitFor();
 }
 const relinkTicket=await send({type:'ticket',adapters:['generic'],label:'Synthetic browser fixture'});
 assert.equal((await send({type:'state'})).state.paired,true,'pending relink must preserve the existing connection');
 const relinkOutput=browserRuntimeDir
  ? execFileSync(path.join(browserRuntimeDir,'serein'),['setup','--request-stdin','--json'],{input:JSON.stringify(relinkTicket),env:nativeEnv,cwd:browserRuntimeDir}).toString()
  : execFileSync('sh',[path.join(root,'skills/serein-context/scripts/connect.sh')],{input:JSON.stringify(relinkTicket),env:nativeEnv}).toString();
 const relinked=JSON.parse(relinkOutput);
 assert.equal(relinked.database_path,setup.database_path,'relink replaced the existing vault');
 await page.waitForFunction(async ()=>{const {state}=await chrome.runtime.sendMessage({type:'state'});return state.paired&&!state.ticket;},{},{timeout:15000});
 await runDashboardRefreshTests({page,requestLogPath:requestLog,root,nativeEnv,runtimeDir:browserRuntimeDir});
 assert.deepEqual(errors,[]);const builtManifest=JSON.parse(await readFile(path.join(extension,'manifest.json')));assert.deepEqual([...builtManifest.permissions].sort(),['alarms','idle','nativeMessaging','storage','tabs']);assert.ok(!builtManifest.content_scripts&&!builtManifest.host_permissions);
 for(const icon of Object.values(builtManifest.icons))await readFile(path.join(extension,icon));
 const result={browser:await context.browser()?.version(),extension_id:id,nativeRuntime:browserRuntimeDir||'skills/serein-context bundled helper via connect.sh and recall.sh',modalInitialSetup:{first:r20Setup,zeroTab:noTabSetup,submitting:submittingSetup},checks:['actual extension loaded','consent off by default','pause survives reload','exclusion persisted before helper connection','automatic native hello pairing','first-run topic-bearing recall question','19 queued events launch no helper','20 events launch one helper','one-minute-old event flushes','browser alarm drains queue with extension page closed','R18 card-scoped correction drafts reset on Escape, close and backdrop','R18 correction confirmation sends only the selected card ID and text','R19 late response cannot mutate a different or reopened dialog','R19 current-instance failure preserves its draft','R20 empty/enabled/submitting modal states and live tabbable recomputation','R20 forward/backward Tab traversal, equal-positive-index DOM order and zero-tabbable fallback','R20 disabled-fieldset, hidden and inert controls excluded','R20 close/Escape focus restoration including a removed trigger','background inertness restored after dialog closure','relink preserves existing connection and vault','T09 dashboard refresh and stale-response suite','popup renders','keyboard focus','light and dark screenshots','six UI languages persist across reload','no console errors','manifest exact permissions','icon paths exist'],nativeMessaging:'PASS: actual sendNativeMessage hello, policy flush, batched ingest ACK, duplicate ACK, skill reader recall, exclude-and-forget',correctionRequests:loggedFeedback.map(request=>({atom_id:request.payload.atom_id,action:request.payload.action,text:request.payload.text}))};await writeFile(resultsPath,JSON.stringify(result,null,2));console.log(result);
}finally{if(releaseMarker)await writeFile(releaseMarker,'cleanup release').catch(()=>{});if(releaseMarker)await new Promise(resolve=>setTimeout(resolve,100));if(context)await context.close();if(registeredPath)await unlink(registeredPath).catch(()=>{});await rm(temp,{recursive:true,force:true})}
