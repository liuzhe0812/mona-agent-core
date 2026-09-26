import http.client
import json
from pathlib import Path
import tempfile
import threading
import unittest

from harness.common import encoded
from harness.engine import Manager
from server.app import create_server
from tests.helpers import ROOT,base_request,wait_run


class Server(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.manager=Manager(ROOT,Path(self.tmp.name))
        self.server=create_server(self.manager,0)
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.token=self.request('GET','/api/bootstrap',auth=False)[1]['token']

    def tearDown(self):
        self.server.shutdown();self.server.server_close();self.thread.join(timeout=3)
        self.manager.close();self.tmp.cleanup()

    def request(self,method,path,body=None,auth=True,headers=None):
        connection=http.client.HTTPConnection('127.0.0.1',self.server.server_port,timeout=5)
        h={'X-Eval-Token':self.token} if auth else {}
        if body is not None:h['Content-Type']='application/json'
        h.update(headers or {})
        connection.request(method,path,encoded(body) if body is not None else None,h)
        response=connection.getresponse();raw=response.read();status=response.status;kind=response.getheader('Content-Type','');connection.close()
        return status,json.loads(raw) if 'application/json' in kind else raw

    def test_local_page_and_catalog(self):
        status,body=self.request('GET','/');self.assertEqual(status,200);self.assertIn('创建测评'.encode(),body)
        status,data=self.request('GET','/api/catalog');self.assertEqual(status,200);self.assertEqual(len(data['tasks']),8)

    def test_auth_and_cross_origin_rejected(self):
        self.assertEqual(self.request('GET','/api/runs',auth=False)[0],403)
        self.assertEqual(self.request('GET','/api/bootstrap',auth=False,headers={'Origin':'https://attacker.invalid'})[0],403)
        self.assertEqual(self.request('GET','/api/bootstrap',auth=False,headers={'Host':'attacker.invalid'})[0],403)
        self.assertEqual(self.request('GET','/',headers={'Sec-Fetch-Site':'cross-site'})[0],403)

    def test_full_http_report_and_compare(self):
        status,run=self.request('POST','/api/runs',base_request(task_ids=['exact']))
        self.assertEqual(status,202);wait_run(self.manager,run['id'])
        status,report=self.request('GET','/api/runs/'+run['id']);self.assertEqual(status,200)
        self.assertEqual(report['status'],'completed');self.assertNotIn('manifest',report)
        status,data=self.request('GET','/api/runs/'+run['id']+'/report');self.assertEqual(status,200);self.assertIn('manifest',data)
        status,evidence=self.request('GET','/api/runs/'+run['id']+'/trials/t-1/evidence');self.assertEqual(status,200);self.assertTrue(evidence['observations'])
        status,other=self.request('POST','/api/runs',base_request(task_ids=['exact']));wait_run(self.manager,other['id'])
        status,diff=self.request('GET',f"/api/compare?left={run['id']}&right={other['id']}");self.assertTrue(diff['comparable'])
        self.assertEqual(self.request('DELETE','/api/runs/'+run['id'])[0],200)

    def test_no_arbitrary_file_or_command_endpoint(self):
        self.assertEqual(self.request('GET','/../../AGENTS.md')[0],404)
        self.assertEqual(self.request('GET','/api/runs/../report')[0],400)
        self.assertEqual(self.request('POST','/api/runs',dict(base_request(),command='rm -rf /'))[0],400)
