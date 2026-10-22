#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Local-only Panelet admin UI preview. All device operations use in-memory fixtures.

This server never connects to a board or NAS and never writes firmware. Restart
it to discard edits. Run: python3 scripts/preview-server-ui.py --port 18744
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import argparse, copy, io, json, threading, time, zipfile

ROOT = Path(__file__).resolve().parents[1]
LOCK = threading.RLock()
TOKEN = 'preview-token'
SAVED_WIFI = ['Studio Wi-Fi', 'NOTE4C Lab']
SCENARIO = 'ready'
READY = True
MODE = 'normal'
PRESENTATION = {'submission_id': 1, 'state': 'presented', 'refreshing': False, 'error': None}
PRESENTATION_STARTED = None
INPUT = {'captured': 0, 'overflow': 0, 'pending': 0}
ACTIVE = 'com.note4c.todo'
JOBS = {}
EVENTS = []
REQUESTS = []
WIDGETS = {}
FILES = {'feeds/todo.json': (ROOT/'examples/widgets/todo/feed.example.json').read_bytes()}

def reset_widgets(empty=False):
    global WIDGETS,ACTIVE,MODE,PRESENTATION,PRESENTATION_STARTED
    demo={'manifest':{'id':'demo.hello','name':'内置状态页','version':'0.1.0','abi':'note4c.rune/3','capabilities':['screen.present'],'host_permissions':[]},'source':'struct Status {}\n#[widget::component] impl Status { pub fn new() { Status {} } #[widget::mounted] pub async fn mounted(self) { let canvas = os::screen::get_context()?; canvas.clear(os::screen::WHITE)?; canvas.text_sized([20, 24], "NOTE4C", os::screen::BLACK, 32)?; canvas.flush().await?; os::runtime::ok(()) } }','resources':[],'built_in':True,'settings':{'interval_seconds':0,'host_permissions':None,'values':{}}}
    WIDGETS={'demo.hello':demo}
    if not empty:
        for directory,title in [('todo','待办事项'),('calendar','月历')]:
            path=ROOT/'examples/widgets'/directory
            manifest=json.loads((path/'manifest.json').read_text());manifest['name']=title
            resources=[{'path':str(p.relative_to(path/'resources')),'text':p.read_text()} for p in (path/'resources').rglob('*') if p.is_file()] if (path/'resources').exists() else []
            WIDGETS[manifest['id']]={'manifest':manifest,'source':(path/'main.rn').read_text(),'resources':resources,'built_in':False,'settings':{'interval_seconds':300 if directory=='todo' else 0,'host_permissions':['https://nas.example.test/note4c/todo.json'] if directory=='todo' else None,'values':{'url':'https://nas.example.test/note4c/todo.json','title':'待办事项','auth_type':'basic','username':'note4c-reader','password':'preview-only'} if directory=='todo' else {}}}
    ACTIVE='demo.hello' if empty else 'com.note4c.todo'
    MODE='normal'
    PRESENTATION={'submission_id':None if empty else 1,'state':'idle' if empty else 'presented','refreshing':False,'error':None}
    PRESENTATION_STARTED=None
    INPUT.update(captured=0,overflow=0,pending=0)

def event(kind,job=None,detail=None):
    EVENTS.append({'seq':len(EVENTS)+1,'kind':kind,'job_id':job,'detail':detail})

def queue(method,params):
    id=len(JOBS)+1
    JOBS[id]={'method':method,'params':copy.deepcopy(params),'started':time.monotonic(),'fail':SCENARIO=='failure','state':'queued','result':None}
    event('job.queued',id)
    return 202,{'job_id':id,'state':'queued'}

def submit_preview(retry=False):
    global PRESENTATION_STARTED
    if not retry:PRESENTATION['submission_id']=(PRESENTATION['submission_id'] or 0)+1
    PRESENTATION.update(state='pending',refreshing=False,error=None)
    PRESENTATION_STARTED=time.monotonic()

def advance_presentation():
    global PRESENTATION_STARTED
    if PRESENTATION_STARTED is None:return
    elapsed=time.monotonic()-PRESENTATION_STARTED
    if elapsed<.2:return
    if elapsed<2.2:PRESENTATION.update(state='presenting',refreshing=True);return
    if SCENARIO=='failure':PRESENTATION.update(state='failed',refreshing=False,error='模拟 BUSY 超时；最新画面保留，可显式重试。')
    else:PRESENTATION.update(state='presented',refreshing=False,error=None)
    PRESENTATION_STARTED=None

def advance():
    global ACTIVE,READY,MODE,PRESENTATION_STARTED
    advance_presentation()
    for id,job in JOBS.items():
        if job['state'] in ('succeeded','failed'):continue
        elapsed=time.monotonic()-job['started']
        if elapsed<.2:continue
        job['state']='running'
        if elapsed<1.4:continue
        if job['fail']:
            job['state']='failed';job['result']={'error':{'code':'preview_failure','message':'模拟任务失败：数据源暂时不可用，请稍后重试。'}};event('job.failed',id);continue
        p=job['params'];method=job['method']
        if method=='widget.configure':WIDGETS[p['id']]['settings']=p['settings'];event('widget.configured',id,p['id'])
        elif method=='widget.activate':ACTIVE=p['id'];event('widget.activated',id,p['id'])
        elif method=='widget.deactivate':ACTIVE='';event('widget.activated',id,'')
        elif method=='wifi.forget':SAVED_WIFI[:]=[name for name in SAVED_WIFI if name!=p['ssid']]
        elif method=='wifi.configure':
            if p['ssid'] not in SAVED_WIFI:SAVED_WIFI.append(p['ssid'])
        elif method=='widget.delete':WIDGETS.pop(p['id'],None)
        elif method=='widget.upload':WIDGETS[p['manifest']['id']]=p;event('widget.uploaded',id,p['manifest']['id'])
        elif method=='storage.initialize':READY=True
        elif method=='system.mode.set':
            MODE=p['mode'];PRESENTATION_STARTED=None
            PRESENTATION.update(submission_id=None,state='idle',refreshing=False,error=None)
            event('system.mode.changed',id,MODE)
            if MODE=='normal' and ACTIVE:submit_preview()
        elif method=='screen.retry':submit_preview(retry=True)
        if method in ('widget.activate','widget.configure') and ACTIVE and MODE=='normal':submit_preview()
        if method=='widget.deactivate':
            PRESENTATION_STARTED=None
            PRESENTATION.update(submission_id=None,state='idle',refreshing=False,error=None)
        job['state']='succeeded';job['result']={'completed':True,'preview':True}
        if method=='wifi.scan':job['result']['networks']=[{'ssid':'Studio Wi-Fi'},{'ssid':'NOTE4C Lab'}]
        event('job.succeeded',id)

def public_widget(w,source=False):
    id=w['manifest']['id'];jobs=[j for j in JOBS.values() if j['params'].get('id')==id and j['state'] in ('queued','running')]
    data={'manifest':w['manifest'],'built_in':w['built_in'],'mutable':not w['built_in'],'resource_count':len(w['resources']),'ram_bytes':len(w['source'].encode()),'state':'busy' if jobs else 'ready','active':id==ACTIVE,'running':MODE=='normal' and any(j['method'] in ('widget.activate','widget.configure') and j['state']=='running' for j in jobs),'interval_seconds':w['settings']['interval_seconds']}
    if source:data.update(source=w['source'],resources=w['resources'])
    return data

def system():
    return {'firmware':'0.1.0 · Preview','free_heap_bytes':8350000,'minimum_free_heap_bytes':7370000,'mode':MODE,'input':copy.deepcopy(INPUT),'presentation':copy.deepcopy(PRESENTATION),'display_busy':PRESENTATION['refreshing'],'main_screen':'widget','storage':{'ready':READY,'kind':'assets_spiffs' if READY else 'uninitialized','persistent':READY},'active_widget':ACTIVE,'network':{'mode':'station','sta_ssid':'Studio Wi-Fi','sta_ip':'192.0.2.42','provisioning':False},'time':{'source':'SNTP / RTC','clock_valid':True,'current_unix_ms':int(time.time()*1000),'configuration':{'ntp_server':'pool.ntp.org','utc_offset_minutes':480}}}

class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args):pass
    def reply(self,status,data,content='application/json; charset=utf-8'):
        raw=data if isinstance(data,bytes) else json.dumps(data,ensure_ascii=False).encode()
        self.send_response(status);self.send_header('Content-Type',content);self.send_header('Content-Length',str(len(raw)));self.send_header('Cache-Control','no-store');self.end_headers();self.wfile.write(raw)
    def do_GET(self):
        with LOCK:
            advance()
            if self.path=='/':
                html=(ROOT/'app/os/src/server.html').read_text().replace('<html lang="zh-CN">','<html lang="zh-CN" data-preview="true">')
                return self.reply(200,html.encode(),'text/html; charset=utf-8')
            if self.path=='/__preview/state':return self.reply(200,{'requests':REQUESTS,'scenario':SCENARIO})
            if not self.authorized():return
            if self.path=='/shared/v1':
                path=self.headers.get('X-Note4c-Path')
                if path:return self.reply(200,FILES[path],'application/octet-stream') if path in FILES else self.reply(404,{'error':{'message':'文件不存在'}})
                return self.reply(200,{'paths':list(FILES),'persistent':READY})
            self.reply(404,{'error':{'message':'接口不存在'}})
    def authorized(self):
        if self.headers.get('X-Note4c-Token')!=TOKEN:self.reply(401,{'error':{'code':-32001,'message':'Admin token 不正确，请到连接设置检查。'}});return False
        if SCENARIO=='offline':self.reply(503,{'error':{'message':'模拟设备连接中断。切换预览场景可以恢复。'}});return False
        return True
    def do_POST(self):
        global SCENARIO,READY,TOKEN
        length=int(self.headers.get('Content-Length','0'))
        if length>65536:return self.reply(413,{'error':{'message':'请求太大'}})
        body=self.rfile.read(length)
        with LOCK:
            advance()
            if self.path=='/__preview/scenario':
                SCENARIO=json.loads(body)['scenario']
                if SCENARIO=='empty':READY=False;reset_widgets(True);JOBS.clear();EVENTS.clear()
                elif SCENARIO=='ready':READY=True;reset_widgets();JOBS.clear();EVENTS.clear()
                return self.reply(200,{'preview':True})
            if not self.authorized():return
            if self.path=='/rpc/v1':
                request=json.loads(body);method=request['method'];p=request.get('params',{});REQUESTS.append(method)
                if method=='dashboard.get':
                    widgets=[public_widget(w) for w in WIDGETS.values()];result={'system':system(),'widgets':widgets,'summary':{'widget_count':len(widgets),'running_count':sum(w['running'] for w in widgets),'default_widget':ACTIVE}};status=200
                elif method=='wifi.saved':status,result=200,{'ssids':SAVED_WIFI,'capacity':5}
                elif method=='system.status':status,result=200,system()
                elif method=='widget.list':status,result=200,{'widgets':[public_widget(w) for w in WIDGETS.values()]}
                elif method=='widget.get':status,result=200,public_widget(WIDGETS[p['id']],True)
                elif method=='widget.configuration':status,result=200,{'id':p['id'],'settings':WIDGETS[p['id']]['settings']}
                elif method=='event.list':status,result=200,{'events':EVENTS[-32:]}
                elif method=='job.get':
                    if p['job_id'] not in JOBS:return self.reply(404,{'error':{'message':'任务不存在'}})
                    j=JOBS[p['job_id']];status,result=200,{'job_id':p['job_id'],'method':j['method'],'state':j['state'],'result':j['result']}
                elif method=='system.mode.set':
                    if p.get('mode') not in ('normal','diagnostic'):return self.reply(400,{'error':{'message':'未知设备模式'}})
                    status,result=queue(method,p)
                elif method=='screen.retry':
                    if PRESENTATION['submission_id'] is None or MODE=='diagnostic':return self.reply(409,{'error':{'message':'当前没有可重试的前台画面'}})
                    status,result=queue(method,p)
                elif method in ('display.text','display.status','test.run') and MODE!='diagnostic':
                    return self.reply(409,{'error':{'message':'请先显式进入诊断模式'}})
                else:status,result=queue(method,p)
                return self.reply(status,{'jsonrpc':'2.0','id':request['id'],'result':result})
            if self.path=='/packages/v1':
                try:
                    z=zipfile.ZipFile(io.BytesIO(body));manifest=json.loads(z.read('manifest.json'));source=z.read('main.rn').decode()
                    if manifest.get('abi')!='note4c.rune/3' or 'entry' in manifest:raise ValueError('unsupported ABI')
                    existing=WIDGETS.get(manifest['id'],{});resources=[{'path':name[10:],'text':z.read(name).decode()} for name in z.namelist() if name.startswith('resources/')]
                    candidate={'manifest':manifest,'source':source,'resources':resources,'built_in':False,'settings':existing.get('settings',{'interval_seconds':0,'host_permissions':None,'values':{}})}
                    status,result=queue('widget.upload',candidate);return self.reply(status,result)
                except Exception:return self.reply(400,{'error':{'message':'无法读取小组件包（.pnw / .n4w）'}})
            if self.path=='/shared/v1':
                path=self.headers.get('X-Note4c-Path')
                if not path:return self.reply(400,{'error':{'message':'缺少路径'}})
                FILES[path]=body;return self.reply(200,{'stored':True,'bytes':len(body)})
            if self.path=='/storage/v1/initialize':
                if READY or json.loads(body).get('confirm')!='ERASE ASSETS':return self.reply(409,{'error':{'message':'无需初始化或确认不正确'}})
                READY=True;return self.reply(200,{'initialized':True})
            if self.path=='/admin/v1/token':TOKEN=json.loads(body)['token'];return self.reply(200,{'updated':True})
            self.reply(404,{'error':{'message':'接口不存在'}})
    def do_DELETE(self):
        with LOCK:
            if not self.authorized():return
            if self.path=='/shared/v1':FILES.pop(self.headers.get('X-Note4c-Path'),None);return self.reply(200,{'deleted':True})
            self.reply(404,{'error':{'message':'接口不存在'}})

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--port',type=int,default=18744);args=parser.parse_args()
    reset_widgets();event('wifi.connected');event('widget.activated',detail=ACTIVE)
    print(f'Panelet UI preview: http://127.0.0.1:{args.port} (simulated device only)',flush=True)
    ThreadingHTTPServer(('127.0.0.1',args.port),Handler).serve_forever()
