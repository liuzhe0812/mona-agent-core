import json
from pathlib import Path
import tempfile
import unittest

from adapters.base import Observation
from graders.rules import grade, verdict
from harness.catalog import Catalog
from harness.common import HarnessError, Redactor, encoded, integer, load_json, read_artifact, safe_path
from tests.helpers import ROOT


class Contracts(unittest.TestCase):
    def test_fixture_catalog_valid(self):
        catalog = Catalog(ROOT)
        self.assertEqual(len(catalog.tasks), 8)
        self.assertEqual(len(catalog.public()['suites']), 2)

    def test_strict_integer(self):
        for value in (True, False, 0, 6, '1', 1.2):
            with self.assertRaises(HarnessError): integer(value, 'n', 1, 5)

    def test_path_boundaries(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            for path in ('../secret', '/etc/passwd', 'C:/secret', 'a\\b', '.', 'a/./b', 'a//b', 'a/../b'):
                with self.assertRaises(HarnessError, msg=path): safe_path(root, path)
            self.assertEqual(safe_path(root, '资料/内容.txt'), root/'资料/内容.txt')

    def test_symlink_refused(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d); (root/'target').write_text('private')
            try: (root/'link').symlink_to(root/'target')
            except OSError: self.skipTest('OS denies unprivileged symlink creation')
            with self.assertRaises(HarnessError): read_artifact(root, 'link')

    def test_artifact_limit(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d); (root/'a').write_bytes(b'x'*20)
            with self.assertRaises(HarnessError): read_artifact(root, 'a', 10)

    def test_json_size_and_nonfinite(self):
        with tempfile.TemporaryDirectory() as d:
            p=Path(d)/'x.json';p.write_text('{"n":NaN}')
            with self.assertRaises(HarnessError): load_json(p)
            p.write_text('x'*30)
            with self.assertRaises(HarnessError): load_json(p, 10)

    def test_redacts_exact_secret_and_headers(self):
        value = Redactor(['real-secret-123']).clean({'output':'real-secret-123 Bearer abcdefghi', 'api_key':'anything'})
        self.assertNotIn('real-secret-123', str(value)); self.assertNotIn('abcdefghi', str(value))
        self.assertEqual(value['api_key'], '[REDACTED]')

    def test_unknown_task_grader_rejected(self):
        task=dict(version=1,id='bad',title='bad',suite='test',turns=[{'prompt':'test'}],checks=[{'type':'eval_python'}])
        with self.assertRaises(HarnessError): Catalog.validate_task(task)

    def test_tool_evidence_missing_not_pass(self):
        checks=grade({'checks':[{'type':'tool_count','name':'read','count':0}]},[Observation('')],Path.cwd())
        self.assertEqual(verdict(checks),'inconclusive')

    def test_grading_not_agent_success_claim(self):
        checks=grade({'checks':[{'type':'contains','value':'correct'}]},[Observation('Everything is done!')],Path.cwd())
        self.assertEqual(verdict(checks),'failed')

    def test_json_boolean_not_integer(self):
        task={'checks':[{'type':'json','value':{'ok':True}}]}
        self.assertEqual(verdict(grade(task,[Observation('{"ok":1}')],Path.cwd())),'failed')
        self.assertEqual(verdict(grade(task,[Observation('{"ok":true}')],Path.cwd())),'passed')

    def test_file_and_unchanged_grading(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);(root/'a').write_text('original')
            task={'fixtures':{'a':'original'},'checks':[{'type':'unchanged','path':'a'},{'type':'absent','path':'extra'}]}
            self.assertEqual(verdict(grade(task,[Observation('')],root)),'passed')
            (root/'a').write_text('modified')
            self.assertEqual(verdict(grade(task,[Observation('')],root)),'failed')

    def test_no_required_checks_not_pass(self):
        self.assertEqual(verdict([{'required':False,'status':'passed'}]), 'inconclusive')
