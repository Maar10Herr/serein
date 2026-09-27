#!/usr/bin/env python3
"""Isolated Firefox Marionette test; no changes to existing Firefox profiles."""
import base64,json,os,pathlib,shutil,socket,subprocess,sys,tempfile,time,uuid,zipfile
ROOT=pathlib.Path(__file__).resolve().parents[1]
firefox_binary=os.environ.get('SEREIN_FIREFOX_BIN') or next((shutil.which(name) for name in ('firefox','firefox-esr','firefox-developer-edition') if shutil.which(name)),None)
if not firefox_binary:
 raise SystemExit('Firefox was not found on PATH; set SEREIN_FIREFOX_BIN to its executable path.')
if sys.platform=='darwin':
 native_directory=pathlib.Path.home()/'Library/Application Support/Mozilla/NativeMessagingHosts'
elif sys.platform.startswith('linux'):
 native_directory=pathlib.Path.home()/'.mozilla/native-messaging-hosts'
else:
 raise SystemExit('This temporary native-manifest harness supports macOS and Linux. Windows registration needs a separate isolated registry fixture.')
with tempfile.TemporaryDirectory(prefix='serein-firefox-',dir=str(pathlib.Path(tempfile.gettempdir()).resolve()),ignore_cleanup_errors=True) as tmp:
 p=pathlib.Path(tmp);profile=p/'profile';profile.mkdir();home=p/'home';home.mkdir();extension_id='serein-context@local.serein';extension_uuid=str(uuid.uuid4())
 port_socket=socket.socket();port_socket.bind(('127.0.0.1',0));port=port_socket.getsockname()[1];port_socket.close()
 prefs={'marionette.port':port,'marionette.enabled':True,'extensions.webextensions.uuids':json.dumps({extension_id:extension_uuid}),'browser.shell.checkDefaultBrowser':False,'browser.startup.homepage_override.mstone':'ignore','datareporting.policy.dataSubmissionEnabled':False}
 (profile/'user.js').write_text('\n'.join('user_pref('+json.dumps(k)+','+json.dumps(v)+');' for k,v in prefs.items()))
 env={**os.environ,'SEREIN_INSTALL_HOME':str(home),'SEREIN_DATA_DIR':str(p/'data'),'MOZ_HEADLESS':'1','MOZ_CRASHREPORTER_DISABLE':'1'}
 log=open(p/'firefox.log','wb');process=subprocess.Popen([firefox_binary,'--headless','--remote-allow-system-access','--no-remote','--profile',str(profile),'--marionette'],stdout=log,stderr=log,env=env)
 sock=None;counter=0;registered=None
 try:
  for _ in range(100):
   try:sock=socket.create_connection(('127.0.0.1',port),.5);break
   except OSError:time.sleep(.1)
  assert sock,'Firefox Marionette did not start';sock.settimeout(20)
  def read():
   head=b''
   while not head.endswith(b':'):head+=sock.recv(1)
   n=int(head[:-1]);data=b''
   while len(data)<n:data+=sock.recv(n-len(data))
   return json.loads(data)
  greeting=read()
  def cmd(name,params={}):
   global counter
   counter+=1;b=json.dumps([0,counter,name,params]).encode();sock.sendall(str(len(b)).encode()+b':'+b);r=read();assert not r[2],r;return r[3]
  session=cmd('WebDriver:NewSession',{'capabilities':{'alwaysMatch':{'acceptInsecureCerts':True}}})
  addon=p/'extension.xpi'
  with zipfile.ZipFile(addon,'w') as z:
   for f in (ROOT/'apps/extension/.output/firefox-mv3').rglob('*'):
    if f.is_file():z.write(f,f.relative_to(ROOT/'apps/extension/.output/firefox-mv3'))
  cmd('Addon:Install',{'path':str(addon),'temporary':True})
  cmd('WebDriver:Navigate',{'url':f'moz-extension://{extension_uuid}/dashboard.html#connections'})
  def script(code,args=[]):return cmd('WebDriver:ExecuteAsyncScript',{'script':code,'args':args,'newSandbox':False,'sandbox':None,'scriptTimeout':15000})['value']
  def send(message):return script('const done=arguments[arguments.length-1]; browser.runtime.sendMessage(arguments[0]).then(done,e=>done({error:e.message}));',[message])
  state=send({'type':'state'});assert state['state']['policy']['consent']==False,state
  send({'type':'policy','patch':{'consent':True,'recall_enabled':True}})
  ticket=send({'type':'ticket','adapters':['generic'],'label':'Synthetic Firefox fixture'})
  setup=json.loads(subprocess.check_output(['sh',str(ROOT/'skills/serein-context/scripts/connect.sh')],input=json.dumps(ticket).encode(),env=env))
  actual=native_directory/'com.serein.context.json'
  actual.parent.mkdir(parents=True,exist_ok=True)
  with actual.open('x') as f:f.write(pathlib.Path(setup['manifest']).read_text())
  registered=actual
  for _ in range(30):
   state=send({'type':'state'})
   if state['state']['paired']:break
   time.sleep(.25)
  assert state['state']['paired'],state
  event={'event_id':str(uuid.uuid4()),'visit_id':str(uuid.uuid4()),'site_key':'example.com','site_epoch':0,'observed_at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'kind':'search','title':'Desk lamp research','search_query':'desk lamp research','foreground_seconds':30}
  request={'protocol':1,'request_id':str(uuid.uuid4()),'source_id':ticket['source_id'],'op':'ingest','capture_epoch':state['state']['policy']['capture_epoch'],'payload':{'events':[event]}}
  response=script('const done=arguments[arguments.length-1]; browser.runtime.sendNativeMessage("com.serein.context",arguments[0]).then(done,e=>done({error:e.message}));',[request]);assert response['acknowledged_ids']==[event['event_id']],response
  response=script('const done=arguments[arguments.length-1]; browser.runtime.sendNativeMessage("com.serein.context",arguments[0]).then(done);',[request]);assert response['duplicate_ids']==[event['event_id']]
  send({'type':'exclude','site':'example.com','forget':True});assert send({'type':'host','op':'dashboard'})['cards']==[]
  send({'type':'pause','paused':True});cmd('WebDriver:Refresh');assert send({'type':'state'})['state']['policy']['paused']
  appearance=script('document.documentElement.dataset.theme="light";const done=arguments[arguments.length-1];const deadline=Date.now()+3000;function check(){const card=document.body;const style=getComputedStyle(card);const result={background:style.backgroundColor,color:style.color};if(result.background==="rgb(247, 246, 242)"||Date.now()>deadline)done(result);else setTimeout(check,100)}check();')
  assert appearance['background']=='rgb(247, 246, 242)',appearance
  screenshot=cmd('WebDriver:TakeScreenshot',{'id':None,'full':True})['value'];(ROOT/'docs/screenshots/firefox-onboarding.png').write_bytes(base64.b64decode(screenshot))
  report={'browser':session.get('capabilities',{}).get('browserVersion'),'nativeMessaging':'PASS','checks':['temporary unsigned installation','consent off by default','actual hello pairing','ingest ACK','duplicate ACK','exclude and forget','persistent pause','screenshot'],'permanent_installation':'requires Mozilla signing'};(ROOT/'docs/firefox-test-results.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
 finally:
  if sock:
   try:cmd('Marionette:Quit',{'flags':['eForceQuit']})
   except Exception:pass
   sock.close()
  process.terminate()
  try:process.wait(timeout=10)
  except subprocess.TimeoutExpired:process.kill();process.wait()
  log.close()
  if registered:registered.unlink(missing_ok=True)
