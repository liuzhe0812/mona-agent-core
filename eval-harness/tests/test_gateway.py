from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
import json
import threading
import time
import unittest
import urllib.error
import urllib.request

from harness.common import Control,encoded
from harness.gateway import ModelGateway


class Gateway(unittest.TestCase):
    def make(self, calls=2,tokens=1000,model=None):
        self.cancel=threading.Event()
        self.gateway=ModelGateway(model or {'kind':'fixture','model':'eval-fixture','protocol':'chat_completions'},
            {'max_model_calls':calls,'max_tokens':tokens},Control(self.cancel,time.monotonic()+20),lambda *_:None)
        self.addCleanup(self.gateway.close)
        return self.gateway

    def request(self,g,auth=True):
        raw=encoded({'model':g.model['model'],'stream':True,'messages':[{'role':'user','content':'Reply with exactly OK'}]})
        request=urllib.request.Request(g.endpoint,data=raw,headers={'Content-Type':'application/json',**({'Authorization':'Bearer '+g.token} if auth else {})})
        try:
            with urllib.request.urlopen(request,timeout=5) as response:return response.status,response.read()
        except urllib.error.HTTPError as exc:return exc.code,exc.read()

    def test_calls_admitted_not_silently_retried(self):
        g=self.make(calls=2)
        self.assertEqual(self.request(g)[0],200);self.assertEqual(self.request(g)[0],200)
        self.assertEqual(self.request(g)[0],429)
        self.assertEqual(g.metrics()['model_calls'],2)
        self.assertEqual(g.metrics()['tokens'],96)

    def test_usage_budget_overshoot_reported(self):
        g=self.make(tokens=10)
        self.assertEqual(self.request(g)[0],200)
        metrics=g.metrics();self.assertEqual(metrics['tokens'],48);self.assertTrue(metrics['budget_error'])
        self.assertEqual(self.request(g)[0],429)

    def test_bad_auth_not_a_model_call(self):
        g=self.make()
        self.assertEqual(self.request(g,False)[0],401);self.assertEqual(g.metrics()['model_calls'],0)

    def test_missing_usage_fail_closed_and_raw_body_unchanged(self):
        captured=[]
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*_):pass
            def do_POST(self):
                raw=self.rfile.read(int(self.headers['Content-Length']));captured.append((raw,self.headers.get('Authorization')))
                data=b'data: {"choices":[{"delta":{"content":"OK"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n'
                self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers();self.wfile.write(data)
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        self.addCleanup(lambda:(server.shutdown(),server.server_close(),thread.join()))
        g=self.make(model={'kind':'remote','model':'selected','protocol':'chat_completions',
            'endpoint':f'http://127.0.0.1:{server.server_port}/custom','key':'upstream-secret'})
        status,raw=self.request(g);self.assertEqual(status,200);self.assertIn(b'OK',raw)
        self.assertIsNone(g.metrics()['tokens']);self.assertFalse(g.metrics()['usage_complete'])
        self.assertEqual(self.request(g)[0],429);self.assertEqual(len(captured),1)
        self.assertEqual(captured[0][1],'Bearer upstream-secret')
        self.assertEqual(json.loads(captured[0][0])['model'],'selected')
        self.assertNotIn('upstream-secret',json.dumps(g.metrics()))

    def test_each_protocol_usage_accounting(self):
        state={};ModelGateway.usage_event({'type':'response.completed','response':{'usage':{'input_tokens':10,'output_tokens':4,'total_tokens':14}}},state)
        self.assertEqual(state['total'],14)
        state={};ModelGateway.usage_event({'type':'message_start','message':{'usage':{'input_tokens':20,'cache_read_input_tokens':5,'output_tokens':0}}},state)
        ModelGateway.usage_event({'type':'message_delta','usage':{'output_tokens':6}},state)
        self.assertEqual(state,{'input':20,'output':6,'cache_read':5})

    def test_cancel_stream_unblocks_request(self):
        g=self.make()
        raw=encoded({'model':'eval-fixture','messages':[{'role':'user','content':'Wait until cancelled'}]})
        result=[]
        def request():
            try:urllib.request.urlopen(urllib.request.Request(g.endpoint,data=raw,headers={'Authorization':'Bearer '+g.token}),timeout=5).read()
            except Exception:pass
            result.append(True)
        thread=threading.Thread(target=request);thread.start();time.sleep(.1);self.cancel.set();thread.join(timeout=3)
        self.assertFalse(thread.is_alive());self.assertTrue(result)
