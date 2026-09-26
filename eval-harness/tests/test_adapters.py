import json
from pathlib import Path
import tempfile
import threading
import time
import unittest

from harness.common import encoded
from harness.engine import Manager
from tests.helpers import ROOT,base_request,wait_run


class Adapters(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.manager=Manager(ROOT,Path(self.tmp.name)/'data')

    def tearDown(self):
        self.manager.close();self.tmp.cleanup()

    def run_command(self, mode, **options):
        self.manager.catalog.agents['command']={'id':'command','label':'protocol fixture','kind':'command',
            'argv':['{python}','-u',str(ROOT/'tests/command_fixture.py'),mode], 'capabilities':['text']}
        req=dict(base_request(task_ids=['exact']),adapter='command',allow_local_execution=True,**options)
        return self.manager.start(req)

    def test_generic_adapter_actual_process(self):
        run=self.run_command('echo');result=wait_run(self.manager,run['id'])
        self.assertEqual(result['trials'][0]['status'],'passed')
        raw=self.manager.evidence(run['id'],'t-1').decode()
        self.assertNotIn('EVAL_MODEL_KEY',raw)
        self.assertEqual(result['mode'],'controlled')

    def test_invalid_protocols_fail(self):
        for mode in ('bad','large','missing','failed','duplicate'):
            with self.subTest(mode=mode):
                run=self.run_command(mode);result=wait_run(self.manager,run['id'])
                self.assertNotEqual(result['trials'][0]['status'],'passed')

    def test_process_timeout(self):
        run=self.run_command('hang',timeout_s=1);result=wait_run(self.manager,run['id'])
        self.assertEqual(result['trials'][0]['status'],'timeout')
        self.assertLess(result['trials'][0]['metrics']['wall_ms'],12000)

    def test_cancel_kills_owned_descendants(self):
        run=self.run_command('child')
        path=Path(self.tmp.name)/'data/runs'/run['id']/'t-1/state/heartbeat.txt'
        deadline=time.monotonic()+10
        while not path.exists() and time.monotonic()<deadline:time.sleep(.05)
        self.assertTrue(path.exists())
        self.manager.cancel(run['id']);result=wait_run(self.manager,run['id'])
        self.assertEqual(result['status'],'cancelled')
        before=path.read_text();time.sleep(.3)
        self.assertEqual(path.read_text(),before,'grandchild must stop, not merely the wrapper process')

    def test_command_without_usage_unknown_for_real_models(self):
        self.manager.catalog.models['loopback']={'id':'loopback','label':'no outbound calls','kind':'remote','model':'fake',
            'protocol':'chat_completions','endpoint_env':'EVAL_TEST_URL','key_env':'EVAL_TEST_KEY'}
        from unittest.mock import patch
        import os
        with patch.dict(os.environ,{'EVAL_TEST_URL':'http://127.0.0.1:1/chat/completions','EVAL_TEST_KEY':'not-a-real-key'}):
            self.manager.catalog.agents['command']={'id':'command','label':'fixture','kind':'command',
                'argv':['{python}','-u',str(ROOT/'tests/command_fixture.py'),'echo'],'capabilities':['text']}
            run=self.manager.start({'adapter':'command','model':'loopback','suite':'all','task_ids':['exact'],'allow_local_execution':True,'allow_paid':True})
            result=wait_run(self.manager,run['id'])
            self.assertEqual(result['trials'][0]['status'],'inconclusive')

    def test_turn_processes_share_only_explicit_trial_state(self):
        self.manager.catalog.tasks['exact']['turns']=[{'prompt':'Reply with exactly EVAL_READY_7'},{'prompt':'Reply with exactly EVAL_READY_7'}]
        run=self.run_command('echo');result=wait_run(self.manager,run['id'])
        self.assertEqual(result['status'],'completed')
        path=Path(self.tmp.name)/'data/runs'/run['id']/'t-1/state/calls.txt'
        self.assertEqual(path.read_text(),'2')
