from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

from harness.common import HarnessError
from harness.engine import Manager
from harness.storage import Store
from tests.helpers import ROOT,base_request,wait_run


class Storage(unittest.TestCase):
    def test_second_owner_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            store=Store(Path(d))
            try:
                with self.assertRaises(HarnessError):Store(Path(d))
            finally:store.close()

    def test_persistent_completed_report(self):
        with tempfile.TemporaryDirectory() as d:
            manager=Manager(ROOT,Path(d))
            rid=manager.start(base_request(task_ids=['exact']))['id'];wait_run(manager,rid);manager.close()
            manager=Manager(ROOT,Path(d))
            try:self.assertEqual(manager.store.get(rid)['status'],'completed')
            finally:manager.close()

    def test_real_process_crash_marks_interrupted_no_replay(self):
        with tempfile.TemporaryDirectory() as d:
            program="from harness.engine import Manager; from pathlib import Path; import sys,time; m=Manager(Path.cwd(),Path(sys.argv[1])); m.catalog.tasks['exact']['turns']=[{'prompt':'Wait until cancelled'}]; r=m.start({'adapter':'selftest','model':'fixture','suite':'all','task_ids':['exact']}); print(r['id'],flush=True); time.sleep(60)"
            process=subprocess.Popen([sys.executable,'-u','-c',program,d],cwd=ROOT,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            try:
                rid=process.stdout.readline().strip();self.assertTrue(rid.startswith('e-'))
                time.sleep(.3);process.kill();process.wait(timeout=5)
                manager=Manager(ROOT,Path(d))
                try:
                    run=manager.store.get(rid)
                    self.assertEqual(run['status'],'interrupted')
                    self.assertEqual(run['summary']['counts'],{'interrupted':1})
                    self.assertEqual(manager.active,{})
                finally:manager.close()
            finally:
                if process.poll() is None:process.kill();process.wait(timeout=5)
                process.stdout.close();process.stderr.close()

    def test_owner_crash_closes_supervised_process_tree(self):
        with tempfile.TemporaryDirectory() as d:
            program="from harness.engine import Manager; from pathlib import Path; import sys,time; m=Manager(Path.cwd(),Path(sys.argv[1])); m.catalog.agents['child']={'id':'child','label':'child','kind':'command','argv':['{python}','-u',str(Path('tests/command_fixture.py').resolve()),'child'],'capabilities':['text']}; r=m.start({'adapter':'child','model':'fixture','suite':'all','task_ids':['exact'],'allow_local_execution':True}); print(r['id'],flush=True); time.sleep(60)"
            process=subprocess.Popen([sys.executable,'-u','-c',program,d],cwd=ROOT,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            try:
                rid=process.stdout.readline().strip();self.assertTrue(rid.startswith('e-'))
                marker=Path(d)/'runs'/rid/'t-1/state/heartbeat.txt'
                end=time.monotonic()+10
                while not marker.exists() and time.monotonic()<end:time.sleep(.05)
                self.assertTrue(marker.exists())
                process.kill();process.wait(timeout=5)
                time.sleep(.5)
                old=marker.read_text();time.sleep(.4)
                self.assertEqual(old,marker.read_text(),'owner EOF must terminate the descendant too')
                manager=Manager(ROOT,Path(d))
                try:self.assertEqual(manager.store.get(rid)['status'],'interrupted')
                finally:manager.close()
            finally:
                if process.poll() is None:process.kill();process.wait(timeout=5)
                process.stdout.close();process.stderr.close()

    def test_event_tail_is_bounded(self):
        with tempfile.TemporaryDirectory() as d:
            manager=Manager(ROOT,Path(d))
            try:
                rid=manager.start(base_request(task_ids=['exact']))['id'];wait_run(manager,rid)
                for i in range(2100):manager.store.event(rid,{'type':'x','time':time.time(),'message':str(i)})
                page=manager.store.events(rid,1)
                self.assertTrue(page['truncated'])
                self.assertLessEqual(len(page['events']),200)
                count=manager.store.db.execute('SELECT COUNT(*) FROM events WHERE run=?',(rid,)).fetchone()[0]
                self.assertEqual(count,2000)
            finally:manager.close()

    def test_corrupt_database_not_reset(self):
        with tempfile.TemporaryDirectory() as d:
            path=Path(d)/'reports.sqlite3';path.write_bytes(b'not a sqlite database')
            with self.assertRaises(Exception):Store(Path(d))
            self.assertEqual(path.read_bytes(),b'not a sqlite database')
