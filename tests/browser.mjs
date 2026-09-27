import {createRequire} from 'node:module';
import {mkdtemp,realpath,rm,mkdir,readFile,writeFile,unlink} from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import path from 'node:path';
import {tmpdir} from 'node:os';
import assert from 'node:assert/strict';
const playwrightPackage=process.env.SEREIN_PLAYWRIGHT_PACKAGE;
const require=createRequire(playwrightPackage?path.resolve(playwrightPackage):import.meta.url);
const {chromium}=require('playwright');
const root=process.cwd(), temp=await realpath(await mkdtemp(path.join(tmpdir(),'serein-browser-')));
const extension=path.join(root,'apps/extension/.output/chrome-mv3');
const evidence=path.join(root,'docs/screenshots');await mkdir(evidence,{recursive:true});
let registeredPath;
let context;
try{
 const launchOptions={headless:true,args:[`--disable-extensions-except=${extension}`,`--load-extension=${extension}`],viewport:{width:1320,height:940},env:{...process.env,SEREIN_DATA_DIR:path.join(temp,'data'),SEREIN_INSTALL_HOME:path.join(temp,'home')}};
 if(process.env.SEREIN_CHROMIUM_BIN)launchOptions.executablePath=process.env.SEREIN_CHROMIUM_BIN;
 context=await chromium.launchPersistentContext(temp,launchOptions);
 let worker=context.serviceWorkers()[0]||await context.waitForEvent('serviceworker');
 const id=new URL(worker.url()).host;
 const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto(`chrome-extension://${id}/dashboard.html#connections`);await page.getByRole('heading',{name:'Connections',exact:true}).waitFor();
 await page.getByRole('link',{name:'https://github.com/Maar10Herr/serein',exact:true}).waitFor();
 await page.getByRole('button',{name:'Copy install request',exact:true}).waitFor();
 await page.screenshot({animations:'disabled',path:path.join(evidence,'onboarding-light.png'),fullPage:true});
 // Exercise consent, persisted policy and private session via real extension messaging.
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
 await send({type:'policy',patch:{consent:true,recall_enabled:true}});
 await send({type:'pause',paused:true});assert.equal((await send({type:'state'})).state.policy.paused,true);
 await page.reload();assert.equal((await send({type:'state'})).state.policy.paused,true);
 await send({type:'pause',paused:false});
 await send({type:'exclude',site:'privacy-check.example.net',forget:true});const after=await send({type:'state'});assert.ok(after.state.policy.excluded_sites.includes('privacy-check.example.net'));assert.ok(after.controls>=1);
 await page.getByRole('button',{name:'Your context',exact:true}).click();await page.getByRole('heading',{name:'Your context',exact:true}).waitFor();await setTheme('light');await page.screenshot({animations:'disabled',path:path.join(evidence,'context-empty-light.png'),fullPage:true});
 await setTheme('dark');await page.screenshot({animations:'disabled',path:path.join(evidence,'context-empty-dark.png'),fullPage:true});
 // Real runtime.sendNativeMessage round trip with a synthetic isolated vault.
 const ticket=await send({type:'ticket',adapters:['generic'],label:'Synthetic browser fixture'});
 const nativeEnv={...process.env,SEREIN_DATA_DIR:path.join(temp,'data'),SEREIN_INSTALL_HOME:path.join(temp,'home')};
 const setup=JSON.parse(execFileSync('sh',[path.join(root,'skills/serein-context/scripts/connect.sh')],{input:JSON.stringify(ticket),env:nativeEnv}).toString());
 assert.equal(setup.status,'ok');
 const manifest=JSON.parse(await readFile(setup.manifest));
 const invocationLog=path.join(temp,'host-invocations.txt');
 const wrapper=path.join(temp,'native-host-wrapper');
 const quote=value=>`'${value.replaceAll("'", "'\\''")}'`;
 await writeFile(wrapper,`#!/bin/sh\nprintf '1\\n' >> ${quote(invocationLog)}\nexec ${quote(manifest.path)} "$@"\n`,{mode:0o700});
 manifest.path=wrapper;
 const nativeCalls=async()=>((await readFile(invocationLog,'utf8').catch(()=>''))).split('\n').filter(Boolean).length;
 const nativeDirectory=path.join(temp,'NativeMessagingHosts');
 await mkdir(nativeDirectory,{recursive:true});
 const candidatePath=path.join(nativeDirectory,'com.serein.context.json');
 await writeFile(candidatePath,JSON.stringify(manifest),{flag:'wx',mode:0o600});registeredPath=candidatePath;
 await page.waitForFunction(async ()=>(await chrome.runtime.sendMessage({type:'state'})).state.paired,{},{timeout:15000});
 const connected=await send({type:'state'});assert.equal(connected.state.paired,true);assert.equal(connected.controls,0,JSON.stringify(connected));
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
 await page.reload();await page.getByRole('heading',{name:'Desk lamp research',exact:true}).waitFor();
 await setTheme('dark');await page.screenshot({animations:'disabled',path:path.join(evidence,'context-populated-dark.png'),fullPage:true});
 await setTheme('light');await page.screenshot({animations:'disabled',path:path.join(evidence,'context-populated-light.png'),fullPage:true});
 const recallRequest={protocol:1,request_id:crypto.randomUUID(),client:'generic',vault:'default',query:'desk lamp research',facets:['desk'],scope:['research'],max_bytes:4096,budget_ms:1500};
 const recalled=JSON.parse(execFileSync('sh',[path.join(root,'skills/serein-context/scripts/recall.sh')],{input:JSON.stringify(recallRequest),env:nativeEnv}).toString());assert.equal(recalled.context.length,1);
 await send({type:'exclude',site:'example.com',forget:true});const forgotten=await send({type:'host',op:'dashboard'});assert.equal(forgotten.cards.length,0);
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
 const relinked=JSON.parse(execFileSync('sh',[path.join(root,'skills/serein-context/scripts/connect.sh')],{input:JSON.stringify(relinkTicket),env:nativeEnv}).toString());
 assert.equal(relinked.database_path,setup.database_path,'relink replaced the existing vault');
 await page.waitForFunction(async ()=>{const {state}=await chrome.runtime.sendMessage({type:'state'});return state.paired&&!state.ticket;},{},{timeout:15000});
 assert.deepEqual(errors,[]);const builtManifest=JSON.parse(await readFile(path.join(extension,'manifest.json')));assert.deepEqual([...builtManifest.permissions].sort(),['alarms','idle','nativeMessaging','storage','tabs']);assert.ok(!builtManifest.content_scripts&&!builtManifest.host_permissions);
 for(const icon of Object.values(builtManifest.icons))await readFile(path.join(extension,icon));
 const result={browser:await context.browser()?.version(),extension_id:id,checks:['actual extension loaded','consent off by default','pause survives reload','exclusion persisted before helper connection','automatic native hello pairing','19 queued events launch no helper','20 events launch one helper','one-minute-old event flushes','browser alarm drains queue with extension page closed','relink preserves existing connection and vault','deletion pending exposed','popup renders','keyboard focus','light and dark screenshots','six UI languages persist across reload','no console errors','manifest exact permissions','icon paths exist'],nativeMessaging:'PASS: actual sendNativeMessage hello, policy flush, batched ingest ACK, duplicate ACK, skill reader recall, exclude-and-forget'};await writeFile('docs/browser-test-results.json',JSON.stringify(result,null,2));console.log(result);
}finally{if(context)await context.close();if(registeredPath)await unlink(registeredPath).catch(()=>{});await rm(temp,{recursive:true,force:true})}
