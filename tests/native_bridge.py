#!/usr/bin/env python3
"""Synthetic process-level Gate A/B tests; no user browser data or profile changes."""
import json,os,pathlib,struct,subprocess,tempfile,time,uuid,statistics
ROOT=pathlib.Path(__file__).resolve().parents[1]
def uid():return str(uuid.uuid4())
with tempfile.TemporaryDirectory(prefix='serein-',dir=str(pathlib.Path(tempfile.gettempdir()).resolve())) as tmp:
    env={**os.environ,'SEREIN_DATA_DIR':tmp+'/data','SEREIN_INSTALL_HOME':tmp+'/home'}
    cli=ROOT/'target/release/serein';host=ROOT/'target/release/serein-host'
    ids=json.loads((ROOT/'release/extension-identity.json').read_text());extension=ids['chrome_extension_id'];source=uid();nonce=uid()+uid()
    def command(args,req=None):
        p=subprocess.run([str(cli),*args,'--json'],input=json.dumps(req).encode() if req is not None else None,capture_output=True,env=env,timeout=5)
        return p,json.loads(p.stdout)
    setup={'protocol':1,'source_id':source,'extension_id':extension,'browser':'chrome','nonce':nonce,'expires_at':int(time.time())+890,'adapters':['generic'],'label':'Synthetic test','consent':True}
    p,r=command(['setup','--request-stdin'],setup);assert p.returncode==0,r
    def native(op,payload,epoch=2,caller=None):
        request={'protocol':1,'request_id':uid(),'source_id':source,'op':op,'capture_epoch':epoch,'payload':payload};b=json.dumps(request).encode();begin=time.perf_counter()
        p=subprocess.run([str(host),caller or 'chrome-extension://'+extension+'/'],input=struct.pack('=I',len(b))+b,capture_output=True,env=env,timeout=2)
        assert p.returncode==0 and len(p.stdout)>=4,(p.returncode,p.stderr)
        n=struct.unpack('=I',p.stdout[:4])[0];assert n==len(p.stdout)-4
        return json.loads(p.stdout[4:]),1000*(time.perf_counter()-begin)
    r,_=native('hello',{'nonce':nonce});assert r['status']=='ok',r
    r,_=native('status',{},caller='chrome-extension://'+'a'*32+'/');assert r['error']['code']=='ACCESS_DENIED'
    policy={'consent':True,'paused':False,'recall_enabled':True,'selected_only':False,'selected_sites':[],'excluded_sites':[],'capture_epoch':2}
    r,_=native('policy.update',policy);assert r['status']=='ok',r
    _,doctor=command(['doctor']);assert doctor['checks']['browser']['state']=='paired',doctor
    assert doctor['checks']['native_host']['state']=='ready',doctor
    assert doctor['checks']['vault']['state']=='ready',doctor
    assert doctor['checks']['recall']['state']=='ready',doctor
    assert doctor['checks']['model']['state']=='ready',doctor
    assert not {'data_root','executable','default_vault','connections'} & doctor.keys(),doctor
    assert tmp not in json.dumps(doctor),doctor
    e={'event_id':uid(),'visit_id':uid(),'site_key':'example.com','site_epoch':0,'observed_at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'kind':'search','title':'Desk lamp research','search_query':'desk lamp research','foreground_seconds':10}
    r,ms=native('ingest',{'events':[e]});assert r['acknowledged_ids']==[e['event_id']],r
    r,_=native('ingest',{'events':[e]});assert r['duplicate_ids']==[e['event_id']] and r['atoms']==1,r
    req={'protocol':1,'request_id':uid(),'client':'generic','vault':'default','query':'desk lamp research','facets':['desk'],'scope':['research'],'max_bytes':4096,'budget_ms':1500}
    p,r=command(['recall','--request-stdin'],req);assert p.returncode==0 and len(r['context'])==1,r
    assert r['context'][0]['subject']=='unknown' and len(p.stdout)<=4097
    r,_=native('forget',{'site':'example.com','atom_id':None,'site_epoch':3});assert r['deleted']==1,r
    r,_=native('ingest',{'events':[e]});assert r['rejected'][0]['reason']=='STALE_SITE_EPOCH',r
    p,r=command(['recall','--request-stdin'],req);assert r['context']==[],r
    samples=[]
    for i in range(20):
        r,ms=native('status',{});samples.append(ms)
    # Malformed / oversized frames return one bounded error then exit.
    for b in [struct.pack('=I',300000),struct.pack('=I',1)+b'\xff',struct.pack('=I',1)+b'{']:
        p=subprocess.run([str(host)],input=b,capture_output=True,env=env,timeout=2);assert json.loads(p.stdout[4:])['status']=='error'
    _,doctor=command(['doctor'])
    report={'passed':['setup isolated user registration','hello caller pairing','wrong caller denied','doctor pairing/host/vault/model/recall checks','save ACK','retry idempotent','bounded recall','forget + replay tombstone','native malformed frames','host exits within 2s'],'status_latency_ms':{'median':statistics.median(samples),'p95':sorted(samples)[18]},'browser_invocation':'not tested: native process harness only','semantic_model':'installed model pack' if doctor['model_available'] else 'absent; explicit lexical fallback'}
    (ROOT/'docs/native-test-results.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
