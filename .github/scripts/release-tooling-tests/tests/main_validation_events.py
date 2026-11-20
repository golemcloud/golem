"""Operator setup and durable event discovery; no validation runs or Slack posts."""

import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[4]
OBSERVER = ROOT / '.github/main-validation/observer'


class EventTests(unittest.TestCase):
    def setUp(self):
        scratch = ROOT / '.amp/in/orb-backup/event-discovery-tests'
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.state = self.directory / 'state'
        for name in ('seen', 'events'):
            (self.state / name).mkdir(parents=True)

    def test_inbox_is_ordered_by_run_and_attempt_and_excludes_seen_and_no_selection(self):
        for run, attempt, status in [(91, 2, 'notified'), (90, 1, 'notified'), (91, 1, 'pending'),
                                     (89, 1, 'no-selection'), (88, 1, 'seen')]:
            (self.state / 'events' / f'{run}-a{attempt}.json').write_text(json.dumps({
                'run': run, 'attempt': attempt, 'status': status}))
        (self.state / 'seen/90-a1.json').write_text('{}')
        output = subprocess.run(['bash', str(OBSERVER / 'list-runs.sh'), '--events'], cwd=ROOT,
                                env={**os.environ, 'OBSERVER_STATE_DIR': str(self.state)},
                                capture_output=True, text=True, check=True, timeout=10)
        self.assertEqual(output.stdout, '91\t1\n91\t2\n')

    def test_init_is_private_and_does_not_replace_existing_configuration(self):
        namespace = runpy.run_path(str(OBSERVER / 'configure-webhook.py'))
        with contextlib.chdir(self.directory), contextlib.redirect_stdout(io.StringIO()), \
                patch.object(sys, 'argv', ['configure-webhook.py', 'init', 'T-existing-owner', str(self.state)]):
            namespace['main']()
            path = self.directory / '.local/main-validation-webhook.json'
            before = path.read_bytes()
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            config = json.loads(before)
            self.assertEqual(config['stateDir'], str(self.state))
            self.assertEqual(len(config['secret']), 64)
            with self.assertRaisesRegex(SystemExit, 'Configuration exists'):
                namespace['main']()
            self.assertEqual(path.read_bytes(), before)

    def test_register_reuses_an_existing_hook_after_an_ambiguous_create(self):
        namespace = runpy.run_path(str(OBSERVER / 'configure-webhook.py'))
        (self.directory / '.local').mkdir()
        config = {'url': 'https://fixture.invalid/capability', 'secret': 'fixture-only'}
        path = self.directory / '.local/main-validation-webhook.json'
        path.write_text(json.dumps(config))
        calls = []

        def github(endpoint, payload=None):
            calls.append((endpoint, payload))
            return [{'id': 17, 'active': True, 'events': ['workflow_run'],
                     'config': {'url': config['url'], 'content_type': 'json', 'insecure_ssl': '0'}}]

        namespace['main'].__globals__['github'] = github
        with contextlib.chdir(self.directory), contextlib.redirect_stdout(io.StringIO()), \
                patch.object(sys, 'argv', ['configure-webhook.py', 'register']):
            namespace['main']()
        self.assertEqual(calls, [('hooks?per_page=100', None)])
        self.assertEqual(json.loads(path.read_text())['hookId'], 17)

    def test_register_refuses_incompatible_existing_hook_transport(self):
        namespace = runpy.run_path(str(OBSERVER / 'configure-webhook.py'))
        (self.directory / '.local').mkdir()
        config = {'url': 'https://fixture.invalid/capability', 'secret': 'fixture-only'}
        path = self.directory / '.local/main-validation-webhook.json'
        path.write_text(json.dumps(config))
        for content_type, insecure_ssl in [('form', '0'), ('json', '1')]:
            with self.subTest(content_type=content_type, insecure_ssl=insecure_ssl):
                def github(endpoint, payload=None):
                    self.assertIsNone(payload)
                    return [{'id': 17, 'active': True, 'events': ['workflow_run'], 'config': {
                        'url': config['url'], 'content_type': content_type, 'insecure_ssl': insecure_ssl}}]

                namespace['main'].__globals__['github'] = github
                with contextlib.chdir(self.directory), patch.object(sys, 'argv', ['configure-webhook.py', 'register']):
                    with self.assertRaisesRegex(SystemExit, 'different settings'):
                        namespace['main']()
                self.assertNotIn('hookId', json.loads(path.read_text()))


if __name__ == '__main__':
    unittest.main()
