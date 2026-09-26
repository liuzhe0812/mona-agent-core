import json
import os
from pathlib import Path
import shutil
import tempfile
import time
import unittest
from unittest.mock import patch

from harness.common import HarnessError, encoded
from harness.engine import Manager
from tests.helpers import ROOT, base_request, wait_run


class Engine(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.manager=Manager(ROOT, Path(self.tmp.name)/'data')

    def tearDown(self):
        self.manager.close();self.tmp.cleanup()

    def test_all_selftests_repeat_isolated(self):
        run=self.manager.start(base_request(repeats=2,concurrency=4))
        result=wait_run(self.manager,run['id'])
        self.assertEqual(result['summary']['counts'],{'passed':16})
        self.assertEqual(result['mode'],'selftest')
        self.assertEqual(result['summary']['model_calls'],0)
        for t in result['trials']:
            self.assertIsNone(t['metrics']['peak_rss_bytes'])
            self.assertIsNone(t['metrics']['disk_io_bytes'])
            evidence=json.loads(self.manager.evidence(run['id'],t['id']))
            self.assertTrue(evidence['observations'])
        count=len(list((Path(self.tmp.name)/'data/runs'/run['id']).glob('*/workspace')))
        self.assertEqual(count,16)

    def test_request_id_reuse_and_conflict(self):
        req=base_request(request_id='repeat-request',task_ids=['exact'])
        a=self.manager.start(req);wait_run(self.manager,a['id'])
        b=self.manager.start(req);self.assertEqual(a['id'],b['id'])
        with self.assertRaises(HarnessError):self.manager.start(dict(req,label='different'))
        self.assertEqual(self.manager.store.list()['total'],1)

    def test_evidence_tamper_not_silent(self):
        run=self.manager.start(base_request(task_ids=['exact']));wait_run(self.manager,run['id'])
        path=Path(self.tmp.name)/'data/runs'/run['id']/'t-1/evidence.json'
        path.write_text('{}')
        with self.assertRaises(HarnessError):self.manager.evidence(run['id'],'t-1')

    def test_compare_warns_different_conditions(self):
        a=self.manager.start(base_request(task_ids=['exact']));wait_run(self.manager,a['id'])
        b=self.manager.start(base_request(task_ids=['structured']));wait_run(self.manager,b['id'])
        result=self.manager.compare(a['id'],b['id'])
        self.assertFalse(result['comparable']);self.assertTrue(result['warnings'])

    def test_grader_change_warns_even_when_tasks_match(self):
        a=self.manager.start(base_request(task_ids=['exact']));a=wait_run(self.manager,a['id'])
        a['provenance']['grader_hash']='older-grader';self.manager.store.save(a)
        b=self.manager.start(base_request(task_ids=['exact']));wait_run(self.manager,b['id'])
        self.assertFalse(self.manager.compare(a['id'],b['id'])['comparable'])

    def test_delete_only_report_environment(self):
        run=self.manager.start(base_request(task_ids=['exact']));wait_run(self.manager,run['id'])
        marker=Path(self.tmp.name)/'untouched';marker.write_text('original')
        self.manager.delete(run['id'])
        self.assertTrue(marker.exists())
        self.assertFalse((Path(self.tmp.name)/'data/runs'/run['id']).exists())
        self.assertEqual(self.manager.store.list()['total'],0)

    def test_missing_capability_skip_not_pass(self):
        self.manager.catalog.agents['selftest']['capabilities']=['text']
        run=self.manager.start(base_request(task_ids=['read-evidence']));result=wait_run(self.manager,run['id'])
        self.assertEqual(result['summary']['counts'],{'skipped':1})
        self.assertEqual(result['summary']['pass_rate'],0)

    def slow(self):
        task=self.manager.catalog.tasks['exact']
        task['turns']=[{'prompt':'Wait until cancelled'}]

    def test_cancellation_and_running_visible(self):
        self.slow();run=self.manager.start(base_request(task_ids=['exact']))
        end=time.monotonic()+3
        while time.monotonic()<end and self.manager.store.get(run['id'])['trials'][0]['status']!='running':time.sleep(.01)
        self.assertEqual(self.manager.store.get(run['id'])['trials'][0]['status'],'running')
        with self.assertRaises(HarnessError):self.manager.delete(run['id'])
        with self.assertRaises(HarnessError):self.manager.start(base_request(task_ids=['exact']))
        self.manager.cancel(run['id']);result=wait_run(self.manager,run['id'])
        self.assertEqual(result['status'],'cancelled');self.assertEqual(result['trials'][0]['status'],'cancelled')

    def test_timeout_is_not_pass(self):
        self.slow();run=self.manager.start(base_request(task_ids=['exact'],timeout_s=1))
        result=wait_run(self.manager,run['id'])
        self.assertEqual(result['trials'][0]['status'],'timeout')

    def test_local_execution_requires_explicit_auth(self):
        self.manager.catalog.agents['cmd']={'id':'cmd','kind':'command','label':'cmd','argv':['{python}','-c','print(1)'],'capabilities':['text']}
        with self.assertRaises(HarnessError) as caught:self.manager.start(dict(base_request(task_ids=['exact']),adapter='cmd'))
        self.assertEqual(caught.exception.code,'authorization')

    def test_paid_requires_explicit_auth_before_start(self):
        self.manager.catalog.agents['cmd']={'id':'cmd','kind':'command','label':'cmd','argv':['{python}'],'capabilities':['text']}
        self.manager.catalog.models['remote']={'id':'remote','kind':'remote','label':'remote','protocol':'chat_completions','model':'test','endpoint_env':'TEST_EVAL_ENDPOINT','key_env':'TEST_EVAL_KEY'}
        with patch.dict(os.environ,{'TEST_EVAL_ENDPOINT':'http://127.0.0.1:1/model','TEST_EVAL_KEY':'secret-not-sent'}):
            with self.assertRaises(HarnessError) as caught:self.manager.start(dict(base_request(task_ids=['exact']),adapter='cmd',model='remote',allow_local_execution=True))
        self.assertEqual(caught.exception.code,'authorization')
        self.assertEqual(self.manager.store.list()['total'],0)

    def test_report_failure_cancels_peers_before_pool_wait(self):
        self.manager.catalog.tasks['structured']['turns']=[{'prompt':'Wait until cancelled'}]
        original=self.manager.store.save
        failed=[False]
        def save(run):
            if not failed[0] and any(t['status']=='passed' for t in run['trials']):
                failed[0]=True
                raise OSError('simulated disk failure')
            return original(run)
        started=time.monotonic()
        with patch.object(self.manager.store,'save',save):
            run=self.manager.start(base_request(task_ids=['exact','structured'],concurrency=2))
            result=wait_run(self.manager,run['id'],timeout=10)
        self.assertTrue(failed[0]);self.assertEqual(result['status'],'interrupted')
        self.assertLess(time.monotonic()-started,10)
        self.assertTrue(all(t['status'] not in ('running','pending') for t in result['trials']))

    def test_failed_file_cleanup_keeps_report_for_retry(self):
        run=self.manager.start(base_request(task_ids=['exact']));wait_run(self.manager,run['id'])
        with patch('harness.engine.shutil.rmtree',side_effect=OSError('locked')):
            with self.assertRaises(OSError):self.manager.delete(run['id'])
        self.assertEqual(self.manager.store.get(run['id'])['status'],'completed')
        self.manager.delete(run['id'])

    def test_strict_request_budgets(self):
        for key,value in [('repeats',0),('concurrency',100),('max_tokens',True),('allow_paid','yes'),('timeout_s',-1)]:
            with self.assertRaises(HarnessError):self.manager.start(base_request(**{key:value}))
