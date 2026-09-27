#!/usr/bin/env python3
"""Reproducible synthetic release-binary timings on the current machine."""
import datetime,json,os,pathlib,platform,statistics,struct,subprocess,sys,tempfile,time,uuid
try:
 import resource
except ImportError:
 resource=None
ROOT=pathlib.Path(__file__).resolve().parents[1]
def uid():return str(uuid.uuid4())
def stats(values):return {'samples':len(values),'median_ms':statistics.median(values),'p95_ms':sorted(values)[int(.95*(len(values)-1))]}
with tempfile.TemporaryDirectory(prefix='serein-perf-',dir=str(pathlib.Path(tempfile.gettempdir()).resolve())) as tmp:
 env={**os.environ,'SEREIN_DATA_DIR':tmp+'/data','SEREIN_INSTALL_HOME':tmp+'/home'};source=uid();nonce=uid()+uid();ext=json.loads((ROOT/'release/extension-identity.json').read_text())['chrome_extension_id'];cli=ROOT/'target/release/serein';host=ROOT/'target/release/serein-host'
 setup={'protocol':1,'source_id':source,'extension_id':ext,'browser':'chrome','nonce':nonce,'expires_at':int(time.time())+890,'adapters':['generic'],'label':'Synthetic performance','consent':True}
 subprocess.run([str(cli),'setup','--request-stdin','--json'],input=json.dumps(setup).encode(),stdout=subprocess.PIPE,check=True,env=env)
 def native(op,payload):
  b=json.dumps({'protocol':1,'request_id':uid(),'source_id':source,'op':op,'capture_epoch':2,'payload':payload}).encode();t=time.perf_counter();p=subprocess.run([str(host),'chrome-extension://'+ext+'/'],input=struct.pack('=I',len(b))+b,stdout=subprocess.PIPE,check=True,env=env,timeout=3);return json.loads(p.stdout[4:]),(time.perf_counter()-t)*1000
 native('hello',{'nonce':nonce});native('policy.update',{'consent':True,'paused':False,'recall_enabled':True,'selected_only':False,'selected_sites':[],'excluded_sites':[],'capture_epoch':2})
 ingest=[]
 for batch in range(25):
  events=[{'event_id':uid(),'visit_id':uid(),'site_key':'research.example.org','site_epoch':0,'observed_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'kind':'search','title':f'desk lamp research J{batch*32+i}','search_query':f'desk lamp research J{batch*32+i}','foreground_seconds':30} for i in range(32)]
  r,elapsed=native('ingest',{'events':events});assert len(r['acknowledged_ids'])==32;ingest.append(elapsed)
 # Finish bounded batches before warm indexed recall measurements.
 for _ in range(12):
  r=json.loads(subprocess.check_output([str(cli),'refresh','--budget-ms','2000','--json'],env=env));
  if r.get('pending_atoms')==0:break
 request={'protocol':1,'request_id':uid(),'client':'generic','vault':'default','query':'desk lamp research','facets':['desk'],'scope':['research'],'max_bytes':4096,'budget_ms':1500}
 recalls=[]
 for _ in range(25):
  request['request_id']=uid();t=time.perf_counter();p=subprocess.run([str(cli),'recall','--request-stdin','--json'],input=json.dumps(request).encode(),capture_output=True,env=env,check=True,timeout=3);recalls.append((time.perf_counter()-t)*1000);r=json.loads(p.stdout);assert len(p.stdout)<=4097 and r['context']
 measured=subprocess.run([str(cli),'recall','--request-stdin','--json'],input=json.dumps(request).encode(),capture_output=True,env=env,check=True)
 child_peak=resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss if resource else None
 rss_bytes=int(child_peak if sys.platform=='darwin' else child_peak*1024) if child_peak is not None else None
 report={'platform':{'Darwin':'macOS','Windows':'Windows','Linux':'Linux'}.get(platform.system(),platform.system()),'architecture':platform.machine(),'build_profile':'release','fixture_atoms':800,'includes_process_launch':True,'cache_state':'warm; no forced OS cache eviction','ingest_32':stats(ingest),'indexed_recall':stats(recalls),'peak_rss_bytes_child_processes_during_harness':rss_bytes,'rss_scope':'Maximum of sequential child processes spawned by this harness, including recall; may exceed the recall process peak.','reference_machine':'This host is not the specified dual-core/4GiB reference laptop.','not_measured':['true cold-cache p95','10000-atom capacity/deadline acceptance','all OS/browser/assistant combinations']}
 (ROOT/'docs/performance-results.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
