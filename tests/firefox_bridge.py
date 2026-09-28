#!/usr/bin/env python3
"""Isolated Firefox Marionette test; no changes to existing Firefox profiles."""
import base64,json,os,pathlib,pwd,shutil,socket,subprocess,sys,tempfile,time,uuid,zipfile
ROOT=pathlib.Path(__file__).resolve().parents[1]
firefox_binary=os.environ.get('SEREIN_FIREFOX_BIN') or next((shutil.which(name) for name in ('firefox','firefox-esr','firefox-developer-edition') if shutil.which(name)),None)
if not firefox_binary:
 raise SystemExit('Firefox was not found on PATH; set SEREIN_FIREFOX_BIN to its executable path.')
if sys.platform=='darwin':
 native_manifest_relative=pathlib.Path('Library/Application Support/Mozilla/NativeMessagingHosts')
elif sys.platform.startswith('linux'):
 native_manifest_relative=pathlib.Path('.mozilla/native-messaging-hosts')
else:
 raise SystemExit('This temporary native-manifest harness supports macOS and Linux. Windows registration needs a separate isolated registry fixture.')
with tempfile.TemporaryDirectory(prefix='serein-firefox-',dir=str(pathlib.Path(tempfile.gettempdir()).resolve()),ignore_cleanup_errors=True) as tmp:
 p=pathlib.Path(tmp);profile=p/'profile';profile.mkdir();browser_home=p/'browser-home';browser_home.mkdir();install_home=p/'install-home';install_home.mkdir();native_directory=browser_home/native_manifest_relative;extension_id='serein-context@local.serein';extension_uuid=str(uuid.uuid4())
 port_socket=socket.socket();port_socket.bind(('127.0.0.1',0));port=port_socket.getsockname()[1];port_socket.close()
 prefs={'marionette.port':port,'marionette.enabled':True,'extensions.webextensions.uuids':json.dumps({extension_id:extension_uuid}),'browser.shell.checkDefaultBrowser':False,'browser.startup.homepage_override.mstone':'ignore','datareporting.policy.dataSubmissionEnabled':False,'ui.systemUsesDarkTheme':1}
 (profile/'user.js').write_text('\n'.join('user_pref('+json.dumps(k)+','+json.dumps(v)+');' for k,v in prefs.items()))
 env={**os.environ,'HOME':str(browser_home),'SEREIN_INSTALL_HOME':str(install_home),'SEREIN_DATA_DIR':str(p/'data'),'MOZ_HEADLESS':'1','MOZ_CRASHREPORTER_DISABLE':'1'}
 log=open(p/'firefox.log','wb');process=subprocess.Popen([firefox_binary,'--headless','--remote-allow-system-access','--no-remote','--profile',str(profile),'--marionette'],stdout=log,stderr=log,env=env)
 sock=None;counter=0;registered=None;registered_identity=None;registration_scope=None;report=None
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
  initial_theme=script('const done=arguments[arguments.length-1];done({systemDark:matchMedia("(prefers-color-scheme: dark)").matches,override:document.documentElement.getAttribute("data-theme"),background:getComputedStyle(document.documentElement).backgroundColor});')
  assert initial_theme=={'systemDark':True,'override':None,'background':'rgb(24, 27, 26)'},initial_theme
  time.sleep(.3)
  screenshot=cmd('WebDriver:TakeScreenshot',{'id':None,'full':True})['value'];(p/'firefox-onboarding-dark.png').write_bytes(base64.b64decode(screenshot))
  state=send({'type':'state'});assert state['state']['policy']['consent']==False,state
  send({'type':'policy','patch':{'consent':True,'recall_enabled':True}})
  cmd('Marionette:SetContext',{'value':'chrome'})
  manifest_root=cmd('WebDriver:ExecuteScript',{'script':'return Services.dirsvc.get("XREUserNativeManifests", Ci.nsIFile).path;','args':[],'newSandbox':False,'sandbox':'system'})['value']
  cmd('Marionette:SetContext',{'value':'content'})
  reported_directory=pathlib.Path(manifest_root)
  expected_home=pathlib.Path(pwd.getpwuid(os.getuid()).pw_dir)
  expected_user_directory=expected_home/native_manifest_relative
  if reported_directory.name=='Mozilla' and (reported_directory/'NativeMessagingHosts').resolve()==expected_user_directory.resolve():
   reported_directory=reported_directory/'NativeMessagingHosts'
  if reported_directory.resolve()==native_directory.resolve():
   native_directory=reported_directory
   registration_scope='temporary Firefox HOME'
  elif reported_directory.resolve()==expected_user_directory.resolve():
   if os.environ.get('SEREIN_FIREFOX_TEST_USER_MANIFEST')!='1':
    raise RuntimeError('Firefox resolves native host manifests via the OS user home; set SEREIN_FIREFOX_TEST_USER_MANIFEST=1 for the guarded test-only registration.')
   if not expected_user_directory.is_dir():
    raise RuntimeError('Expected user-level Mozilla native messaging directory is absent; refusing to create browser configuration directories.')
   native_directory=reported_directory
   registration_scope='temporary user-level Firefox host manifest'
  else:
   raise RuntimeError('Firefox reported an unexpected native host manifest directory; refusing to write a manifest.')
  ticket=send({'type':'ticket','adapters':['generic'],'label':'Synthetic Firefox fixture'})
  setup=json.loads(subprocess.check_output(['sh',str(ROOT/'skills/serein-context/scripts/connect.sh')],input=json.dumps(ticket).encode(),env=env))
  manifest=json.loads(pathlib.Path(setup['manifest']).read_text());invocation_log=p/'native-host-invocations.txt';wrapper=p/'native-host-wrapper'
  quote=lambda value:"'"+str(value).replace("'","'\\''")+"'"
  wrapper.write_text(f"#!/bin/sh\nprintf '1\\n' >> {quote(invocation_log)}\nexec {quote(manifest['path'])} \"$@\"\n");wrapper.chmod(0o700);manifest['path']=str(wrapper)
  actual=native_directory/'com.serein.context.json'
  if os.path.lexists(actual):
   raise RuntimeError('Refusing Firefox test registration because com.serein.context.json already exists.')
  try:
   with actual.open('x') as f:
    registered=actual
    actual.chmod(0o600)
    registered_stat=os.fstat(f.fileno())
    registered_identity=(registered_stat.st_dev,registered_stat.st_ino)
    json.dump(manifest,f)
  except FileExistsError:
   raise RuntimeError('Refusing Firefox test registration because com.serein.context.json already exists.') from None
  for _ in range(30):
   state=send({'type':'state'})
   if state['state']['paired']:break
   time.sleep(.25)
  invocations=len(invocation_log.read_text().splitlines()) if invocation_log.exists() else 0
  assert state['state']['paired'],{'paired':state['state']['paired'],'controls':state.get('controls'),'ticket_present':bool(state['state'].get('ticket')),'native_host_invocations':invocations,'allowed_extensions':manifest.get('allowed_extensions')}
  event={'event_id':str(uuid.uuid4()),'visit_id':str(uuid.uuid4()),'site_key':'example.com','site_epoch':0,'observed_at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'kind':'search','title':'Desk lamp research','search_query':'desk lamp research','foreground_seconds':30}
  request={'protocol':1,'request_id':str(uuid.uuid4()),'source_id':ticket['source_id'],'op':'ingest','capture_epoch':state['state']['policy']['capture_epoch'],'payload':{'events':[event]}}
  response=script('const done=arguments[arguments.length-1]; browser.runtime.sendNativeMessage("com.serein.context",arguments[0]).then(done,e=>done({error:e.message}));',[request]);assert response['acknowledged_ids']==[event['event_id']],response
  response=script('const done=arguments[arguments.length-1]; browser.runtime.sendNativeMessage("com.serein.context",arguments[0]).then(done);',[request]);assert response['duplicate_ids']==[event['event_id']]
  send({'type':'exclude','site':'example.com','forget':True});assert send({'type':'host','op':'dashboard'})['cards']==[]
  send({'type':'pause','paused':True});cmd('WebDriver:Refresh');assert send({'type':'state'})['state']['policy']['paused']
  appearance=script('document.documentElement.dataset.theme="light";const done=arguments[arguments.length-1];const deadline=Date.now()+3000;function check(){const card=document.body;const style=getComputedStyle(card);const result={background:style.backgroundColor,color:style.color};if(result.background==="rgb(247, 246, 242)"||Date.now()>deadline)done(result);else setTimeout(check,100)}check();')
  assert appearance['background']=='rgb(247, 246, 242)',appearance
  time.sleep(.3)
  screenshot=cmd('WebDriver:TakeScreenshot',{'id':None,'full':True})['value'];(p/'firefox-onboarding.png').write_bytes(base64.b64decode(screenshot))
  report={'browser':session.get('capabilities',{}).get('browserVersion'),'nativeMessaging':'PASS','nativeManifestRegistration':registration_scope,'checks':['temporary unsigned installation','consent off by default','actual hello pairing','ingest ACK','duplicate ACK','exclude and forget','persistent pause','system dark theme default','screenshot'],'permanent_installation':'requires Mozilla signing'}
 finally:
  if sock:
   try:cmd('Marionette:Quit',{'flags':['eForceQuit']})
   except Exception:pass
   sock.close()
  process.terminate()
  try:process.wait(timeout=10)
  except subprocess.TimeoutExpired:process.kill();process.wait()
  log.close()
  if registered:
   try:
    current=registered.lstat()
    if (current.st_dev,current.st_ino)!=registered_identity:
     raise RuntimeError('Firefox test manifest path changed during the run; preserving the replacement file.')
    registered.unlink()
    if registration_scope=='temporary user-level Firefox host manifest':report['nativeManifestCleanup']='created exclusively and removed'
   except FileNotFoundError:
    if registration_scope=='temporary user-level Firefox host manifest':report['nativeManifestCleanup']='file was already absent at cleanup'
  if report is not None:
   (ROOT/'docs/screenshots/firefox-onboarding-dark.png').write_bytes((p/'firefox-onboarding-dark.png').read_bytes())
   (ROOT/'docs/screenshots/firefox-onboarding.png').write_bytes((p/'firefox-onboarding.png').read_bytes())
   (ROOT/'docs/firefox-test-results.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
