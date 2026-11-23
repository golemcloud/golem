import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[4]
spec = importlib.util.spec_from_file_location(
    'paired_benchmarks', ROOT / 'integration-tests/benchmark_suites/paired_benchmarks.py')
paired = importlib.util.module_from_spec(spec)
spec.loader.exec_module(paired)


def run(source, values):
    return dict(schema_version=2, harness_version=1,
                run_id=source, runner=dict(id='ci'), suite_fingerprint='fixed-suite',
                artifacts=dict(runner=dict(sha256='fixed-harness')),
                source=dict(repository='golemcloud/golem', commitSha=source),
                results=[dict(name=name, iterations=3, entry_version=1, series_key='same-environment',
                              results=[dict(run_config=dict(clusterSize=1, size=7, length=11),
                                            iteration_results=[dict(index=i, failures={},
                                                duration_samples=dict(latency=[value, value, value * 100]))
                                                               for i in range(3)])])
                         for name, value in values.items()])


class PairedBenchmarkTest(unittest.TestCase):
    def records(self):
        records = []
        for pair, side in paired.schedule(3):
            values = (dict(improved=20, regressed=40, base_only=3) if side == 'base'
                      else dict(improved=10, regressed=60, head_only=9))
            record = run(side, values)
            record['run_id'] = f'{pair}-{side}'
            records.append(dict(pair=pair, side=side, run=record))
        return records

    def test_interleaving_direction_missing_entries_and_intervals(self):
        records = self.records()
        self.assertEqual([item['side'] for item in records], ['base', 'head'] * 3)
        report = paired.compare(records, 'base', 'head')
        results = {item['entry']: item for item in report['results']}
        self.assertEqual(results['improved']['status'], 'improvement')
        self.assertEqual(results['improved']['relative_change'], -0.5)
        self.assertEqual(results['improved']['confidence_interval'], [-0.5, -0.5])
        self.assertEqual(results['regressed']['status'], 'regression')
        self.assertEqual(results['regressed']['relative_change'], 0.5)
        self.assertEqual(results['regressed']['confidence_interval'], [0.5, 0.5])
        self.assertEqual(results['improved']['base_observations'], [20, 20, 20])
        self.assertEqual(results['base_only']['status'], 'missing-in-head')
        self.assertNotIn('confidence_interval', results['base_only'])
        self.assertEqual(results['head_only']['status'], 'missing-in-base')
        grouped = records[::2] + records[1::2]
        with self.assertRaises(ValueError):
            paired.compare(grouped, 'base', 'head')

    def test_changed_workload_or_environment_is_not_compared(self):
        for field in ['series_key', 'entry_version']:
            records = self.records()
            for item in records[1::2]:
                for entry in item['run']['results']:
                    entry[field] = 'different' if field == 'series_key' else 2
            results = paired.compare(records, 'base', 'head')['results']
            self.assertTrue(results)
            self.assertTrue(all('confidence_interval' not in item for item in results))

    def test_failed_missing_or_misidentified_evidence_is_rejected(self):
        for mutation in ['failure', 'missing-iteration', 'missing-metric', 'source', 'schema',
                         'duplicate-process', 'runner', 'harness', 'suite', 'unknown-suite', 'unknown-series']:
            records = copy.deepcopy(self.records())
            run = records[1]['run']
            iterations = run['results'][0]['results'][0]['iteration_results']
            if mutation == 'failure':
                iterations[1]['failures'] = dict(invocation=['failed'])
            elif mutation == 'missing-iteration':
                iterations.pop()
            elif mutation == 'missing-metric':
                iterations[1]['duration_samples'] = {}
            elif mutation == 'source':
                run['source']['commitSha'] = 'other'
            elif mutation == 'schema':
                run['schema_version'] = 1
            elif mutation == 'duplicate-process':
                run['run_id'] = records[0]['run']['run_id']
            elif mutation == 'runner':
                run['runner']['id'] = 'other'
            elif mutation == 'harness':
                run['artifacts']['runner']['sha256'] = 'other'
            elif mutation == 'unknown-suite':
                for item in records:
                    item['run']['suite_fingerprint'] = None
            elif mutation == 'unknown-series':
                for item in records:
                    item['run']['results'][0]['series_key'] = None
            else:
                run['suite_fingerprint'] = 'other'
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                paired.compare(records, 'base', 'head')

    def test_iteration_observations_are_not_weighted_by_raw_request_count(self):
        records = self.records()
        for item in records:
            iterations = item['run']['results'][0]['results'][0]['iteration_results']
            groups = [[2] * 100, [10], [20]] if item['side'] == 'base' else [[3], [5], [100] * 100]
            for iteration, group in zip(iterations, groups):
                iteration['duration_samples']['latency'] = group
        result = next(item for item in paired.compare(records, 'base', 'head')['results']
                      if item['entry'] == 'improved')
        self.assertEqual(result['base_observations'], [10, 10, 10])
        self.assertEqual(result['head_observations'], [5, 5, 5])
        self.assertEqual(result['relative_change'], -0.5)

    def test_relative_changes_keep_the_pairing_when_the_host_drifts(self):
        records = self.records()
        for item in records:
            value = ([1, 100, 10000] if item['side'] == 'base' else [2, 110, 12000])[item['pair']]
            for iteration in item['run']['results'][0]['results'][0]['iteration_results']:
                iteration['duration_samples']['latency'] = [value]
        result = next(item for item in paired.compare(records, 'base', 'head')['results']
                      if item['entry'] == 'improved')
        self.assertAlmostEqual(result['relative_change'], 0.2)
        self.assertAlmostEqual(result['confidence_interval'][0], 0.1)
        self.assertAlmostEqual(result['confidence_interval'][1], 1.0)
        self.assertEqual(result['status'], 'regression')

    def test_execution_retains_order_and_stops_without_replacement_on_failure(self):
        scratch = ROOT / '.amp/in/orb-backup'
        scratch.mkdir(parents=True, exist_ok=True)
        for fail, leftover, wrong_origin in [(False, False, False), (True, False, False),
                                             (False, True, False), (False, False, True)]:
            with self.subTest(fail=fail, leftover=leftover, wrong_origin=wrong_origin), tempfile.TemporaryDirectory(dir=scratch) as directory:
                folder = Path(directory)
                fake_bin = folder / 'bin'
                fake_bin.mkdir()
                docker = fake_bin / 'docker'
                docker.write_text('''#!/usr/bin/env python3
import pathlib, sys
root = pathlib.Path(__file__).resolve().parents[1]
if sys.argv[1:] == ['ps', '-q'] and (root / 'leftover').exists() and (root / 'order').exists():
    print('unrelated-container')
if sys.argv[1] == 'stop':
    (root / 'unrelated-stopped').touch()
''')
                docker.chmod(0o755)
                command = ['python3', str(ROOT / 'integration-tests/benchmark_suites/paired_benchmarks.py'), 'run']
                for side in ('base', 'head'):
                    root = folder / side
                    root.mkdir()
                    subprocess.run(['git', 'init', '--quiet', str(root)], check=True)
                    (root / 'source').write_text(side)
                    subprocess.run(['git', '-C', str(root), 'add', 'source'], check=True)
                    subprocess.run(['git', '-C', str(root), '-c', 'user.name=Fixture', '-c',
                                    'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', side], check=True)
                    source = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
                    origin = folder / f'{side}-origin.json'
                    origin.write_text(json.dumps(dict(head_sha=source, event='workflow_dispatch',
                                                      path='.github/workflows/other.yaml' if wrong_origin else '.github/workflows/benchmark.yaml',
                                                      run_attempt=1, conclusion='success')))
                    command += [f'--{side}-root', str(root), f'--{side}-source', source,
                                f'--{side}-origin', str(origin)]
                    (root / 'target/release').mkdir(parents=True)
                    for name in ('worker-executor', 'golem-worker-service', 'golem-registry-service',
                                 'golem-shard-manager', 'golem-component-compilation-service'):
                        (root / 'target/release' / name).write_text(f'not executable: {side} {name}')
                    (root / 'test-components').mkdir()
                    (root / 'test-components/fixture.wasm').write_text('not wasm')
                    (root / 'builtin-artifacts.json').write_text('{}')
                harness = folder / 'head/target/benchmarks/benchmarks'
                harness.parent.mkdir()
                template = run('unused', dict(improved=10))
                harness.write_text('''#!/usr/bin/env python3
import hashlib, json, pathlib, sys
root = pathlib.Path.cwd()
def option(name): return sys.argv[sys.argv.index(name) + 1]
with (root.parent / 'order').open('a') as stream: stream.write(root.name + '\\n')
if root.name == 'head' and (root.parent / 'fail-head').exists(): sys.exit(7)
record = json.loads(''' + repr(json.dumps(template)) + ''')
record.update(run_id=option('--run-id'), runner=dict(id=option('--runner-id')))
record['source']['commitSha'] = option('--source-commit-sha')
for result in record['results'][0]['results'][0]['iteration_results']:
    result['duration_samples']['latency'] = [20 if root.name == 'base' else 10]
def artifact(path): return dict(path=str(path), sha256=hashlib.sha256(path.read_bytes()).hexdigest())
record['artifacts'] = dict(runner=artifact(pathlib.Path(sys.argv[0])),
    services={('golem-worker-executor' if p.name == 'worker-executor' else p.name): artifact(p)
              for p in (root / 'target/release').iterdir()},
    fixtures=dict(fixture=artifact(root / 'test-components/fixture.wasm')))
pathlib.Path(option('--save-to-json')).write_text(json.dumps(dict(runs=[record])))
''')
                harness.chmod(0o755)
                suite = folder / 'suite.yaml'
                suite.write_text('synthetic runner only; no performance workload')
                if fail:
                    (folder / 'fail-head').touch()
                if leftover:
                    (folder / 'leftover').touch()
                output = folder / 'results'
                command += ['--suite', str(suite), '--pairs', '3', '--deadline', str(int(time.time()) + 300),
                            '--output', str(output)]
                environment = {**os.environ, 'GITHUB_ACTIONS': 'true', 'GITHUB_RUN_ID': 'synthetic',
                               'PATH': f'{fake_bin}:{os.environ["PATH"]}', 'PYTHONDONTWRITEBYTECODE': '1'}
                result = subprocess.run(command, env=environment, text=True, capture_output=True, timeout=20)
                if wrong_origin:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse((folder / 'order').exists())
                    self.assertFalse((output / 'comparison.json').exists())
                    continue
                order = (folder / 'order').read_text().splitlines()
                self.assertFalse((folder / 'unrelated-stopped').exists())
                if leftover:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(order, ['base'])
                    status = json.loads((output / 'process-0-base.status.json').read_text())
                    self.assertFalse(status['valid'])
                    self.assertEqual(status['remaining_containers'], ['unrelated-container'])
                    self.assertFalse(json.loads((output / 'ledger-0.json').read_text())['complete'])
                    self.assertFalse((output / 'comparison.json').exists())
                elif fail:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(order, ['base', 'head'])
                    self.assertTrue((output / 'process-0-base.json').exists())
                    status = json.loads((output / 'process-1-head.status.json').read_text())
                    self.assertFalse(status['valid'])
                    self.assertEqual(status['exit_code'], 7)
                    self.assertFalse(json.loads((output / 'ledger-1.json').read_text())['complete'])
                    self.assertFalse((output / 'comparison.json').exists())
                else:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(order, ['base', 'head'] * 3)
                    report = json.loads((output / 'comparison.json').read_text())
                    self.assertEqual(report['results'][0]['relative_change'], -0.5)
                    offline = subprocess.run([command[0], command[1], 'compare', str(output / 'ledger-5.json'),
                                              '--output', str(output / 'reanalysis.json')], env=environment,
                                             text=True, capture_output=True, timeout=20)
                    self.assertEqual(offline.returncode, 0, offline.stderr)
                    self.assertEqual(json.loads((output / 'reanalysis.json').read_text()), report)


if __name__ == '__main__':
    unittest.main()
