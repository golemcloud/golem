import collections
import copy
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[4]
SCRIPT = ROOT / 'integration-tests/benchmark_suites/validate_r3_metrics.py'
spec = importlib.util.spec_from_file_location('r3_metrics', SCRIPT)
r3 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r3)
sys.path.insert(0, str(SCRIPT.parent))
import r3_selector as selector
import r3_campaign as campaign


def configuration(name='throughput-cpu-intensive', cluster=5, size=1, length=100):
    config = dict(clusterSize=cluster, size=size, length=length, disableCompilationCache=False)
    return dict(entry=name, config=config, iterations=3, version=1,
                **r3.metric_rules(name, config))


def iteration(specification, index=0, route='local'):
    """Construct wire samples, not aggregate summaries, with known invocation multiplicities."""
    result = dict(index=index, failures={}, duration_samples={}, count_samples={})
    for key, rule in specification['metrics'].items():
        if rule['role'] == 'conditional-rpc':
            continue
        values = [rule.get('expected', 0 if rule.get('zero') else 10)] * rule['samples']
        field = 'duration_samples' if rule['quantity'] == 'duration' else 'count_samples'
        result[field][key] = values
    if specification['entry'] in r3.CALLS:
        calls = {'throughput-echo': 250, 'throughput-large-input': 100,
                 'throughput-cpu-intensive': 10}[specification['entry']]
        size = specification['config']['size']
        for language in ('rust', 'ts'):
            prefix = f'{language}-agent-rpc-{route}-'
            result['duration_samples'][prefix + 'invocation'] = [10] * (size * calls)
            for suffix in ('retries', 'timeouts'):
                result['count_samples'][prefix + 'invocation-' + suffix] = [0] * (size * calls)
            for worker in range(size):
                result['duration_samples'][prefix + f'worker-{worker}__secondary'] = [10] * calls
                for suffix in ('retries', 'timeouts'):
                    result['count_samples'][prefix + f'worker-{worker}-{suffix}__secondary'] = [0] * calls
    return result


def suite_record(declaration, run_id='saved-1'):
    entries = {}
    hardware = 'a' * 64
    for item in declaration['configurations']:
        series = hashlib.sha256(json.dumps([1, item['version'], r3.RUNNER, hardware, r3.STORAGE, False],
                                          separators=(',', ':')).encode()).hexdigest()
        entry = entries.setdefault(item['entry'], dict(name=item['entry'], entry_version=item['version'], iterations=item['iterations'],
            series_key=series, measurement_environment=dict(runner_id=r3.RUNNER,
                hardware_fingerprint=hardware, storage_profile=r3.STORAGE, sockets_enabled=False), results=[]))
        entry['results'].append(dict(run_config=item['config'],
                                     iteration_results=[iteration(item, i) for i in range(item['iterations'])]))
    return dict(schema_version=2, harness_version=1, suite=declaration['suite_name'], suite_fingerprint=declaration['suite_fingerprint'],
                run_id=run_id, source=dict(repository='golemcloud/golem', commitSha=declaration['source']),
                runner=dict(id=r3.RUNNER), results=list(entries.values()),
                artifacts=dict(runner=dict(sha256='b' * 64),
                               services={name: dict(sha256='c' * 64) for name in r3.SERVICES},
                               fixtures={name: dict(sha256='d' * 64) for name in declaration['fixtures']}))


class R3MetricsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        directory = tempfile.TemporaryDirectory()
        cls.addClassCleanup(directory.cleanup)
        cls.producer_root = Path(directory.name)
        # Exact producer files from SOURCE, not the changing harness checkout.
        with tarfile.open(Path(__file__).with_name('r3-producers.tar.gz')) as archive:
            for name in r3.PRODUCERS:
                path = cls.producer_root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.extractfile(name).read())
        cls.declaration = r3.manifest(cls.producer_root)

    def test_inventory_is_source_defined_including_unobserved_route(self):
        items = self.declaration['configurations']
        self.assertEqual(len(items), 84)
        roles = collections.Counter(rule['role'] for s in items for rule in s['metrics'].values())
        self.assertEqual(roles, {'fixed': 1099, 'conditional-rpc': 264, 'selected-storage': 44})
        item = next(s for s in items if s['entry'] == 'throughput-large-input'
                    and s['config'] == dict(clusterSize=5, size=1, length=10000, disableCompilationCache=False))
        self.assertFalse(item['metrics']['rust-agent-rpc-local-invocation']['structural_na'])
        self.assertIn('rust-agent-rpc-local-invocation-retries', item['metrics'])
        aggregate = configuration('throughput-large-input-aggregate')
        self.assertIn('rust-agent-rpc-batch-duration', aggregate['metrics'])
        self.assertNotIn('rust-agent-rpc-local-invocation', aggregate['metrics'])

    def test_absent_route_is_na_only_after_complete_accounting(self):
        s = configuration(cluster=1)
        raw = iteration(s)
        view = r3.iteration_view(raw, s, set())
        self.assertTrue(view['valid'])
        self.assertEqual(view['index'], 0)
        self.assertEqual(view['exposures'], {'rust/local': 10, 'rust/remote': 0, 'ts/local': 10, 'ts/remote': 0})
        remote = view['observations']['rust-agent-rpc-remote-invocation']
        self.assertEqual((remote['state'], remote['value'], remote['reason']),
                         ('not-applicable', None, 'structurally-impossible'))
        changes = ('primary-loss', 'target-loss', 'duplicate-target', 'unknown-target', 'missing-rate',
                   'retry', 'secondary-retry', 'timeout', 'unclassified', 'recovered', 'failure', 'skip')
        for change in changes:
            damaged = copy.deepcopy(raw)
            d, c = damaged['duration_samples'], damaged['count_samples']
            if change == 'primary-loss':
                d['rust-agent-rpc-local-invocation'].pop()
            elif change == 'target-loss':
                del d['rust-agent-rpc-local-worker-0__secondary']
            elif change == 'duplicate-target':
                d['rust-agent-rpc-remote-worker-0__secondary'] = [10] * 10
            elif change == 'unknown-target':
                d['rust-agent-rpc-local-worker-1__secondary'] = d.pop('rust-agent-rpc-local-worker-0__secondary')
            elif change == 'missing-rate':
                del c['rust-agent-rpc-throughput-ops-per-sec']
            elif change == 'retry':
                c['rust-agent-rpc-local-invocation-retries'][0] = 1
            elif change == 'secondary-retry':
                c['rust-agent-rpc-local-worker-0-retries__secondary'][0] = 1
            elif change == 'timeout':
                c['rust-agent-rpc-local-invocation-timeouts'].pop()
            elif change == 'unclassified':
                d['rust-agent-rpc-invocation'] = [10]
            elif change == 'recovered':
                d['rust-agent-rpc-local-invocation-recovered'] = [10]
            elif change == 'failure':
                damaged['failures'] = {'cleanup': ['failed']}
            else:
                damaged['skipped'] = 'previous setup failed'
            with self.subTest(change=change):
                self.assertFalse(r3.iteration_view(damaged, s, set())['valid'])

    def test_phase_witnesses_guard_sparse_zero_and_keep_operation_labels_distinct(self):
        s = configuration('streaming-rpc-reconnect', size=129, length=64)
        raw = iteration(s)
        get = 'storage-completion-keyvalue-get-stream_session_index-read_recovery-page'
        many = 'storage-completion-keyvalue-get_many-stream_session_index-read_recovery-page'
        raw['count_samples'][many] = [2]
        before = copy.deepcopy(raw)
        view = r3.iteration_view(raw, s, {get})
        self.assertTrue(view['valid'])
        self.assertEqual(view['observations'][get]['state'], 'producer-zero')
        self.assertEqual(view['observations'][get]['value'], 0)
        self.assertEqual(view['observations'][many]['value'], 2)
        self.assertEqual(raw, before)
        del raw['count_samples'][many]
        view = r3.iteration_view(raw, s, {get})
        self.assertFalse(view['valid'])
        self.assertEqual(view['observations'][get]['state'], 'invalid')
        self.assertIsNone(r3.sparse_phase('storage-startup-keyvalue-get-worker-read-metadata', s['sparse_phases']))

    def test_fixed_metrics_numeric_domains_and_rates_are_not_zero_extended(self):
        s = configuration()
        for change in ('missing-fixed', 'nan', 'negative', 'boolean', 'fractional-count', 'unknown', 'rate-vector'):
            raw = iteration(s)
            if change == 'missing-fixed':
                del raw['duration_samples']['rust-agent-invocation']
            elif change in ('nan', 'negative', 'boolean'):
                raw['duration_samples']['rust-agent-invocation'][0] = {'nan': float('nan'), 'negative': -1, 'boolean': True}[change]
            elif change == 'fractional-count':
                raw['count_samples']['rust-agent-invocation-retries'][0] = 0.5
            elif change == 'unknown':
                raw['count_samples']['storage-made-up-family'] = [1]
            else:
                raw['count_samples']['rust-agent-throughput-ops-per-sec'] = [10, 20]
            with self.subTest(change=change):
                self.assertFalse(r3.iteration_view(raw, s, set())['valid'])
        self.assertEqual(r3.noise([0] * 5, 'rate')['status'], 'under-resolution')
        self.assertEqual(r3.noise([0] * 5, 'duration')['status'], 'under-resolution')
        self.assertEqual(r3.noise([0] * 5, 'event-count')['sample_cv'], 0)
        self.assertEqual(r3.noise([0, 0, 1, 1, 1], 'event-count')['status'], 'noisy')
        self.assertEqual(r3.noise([10] * 4, 'duration')['status'], 'insufficient-coverage')
        self.assertEqual(r3.noise([10] * 5, 'duration')['status'], 'within-noise-threshold')
        control = r3.noise([90, 100, 100, 100, 110], 'duration')
        self.assertEqual(control['status'], 'within-noise-threshold')
        self.assertFalse(control['comparable_control_within_threshold'])

    def test_process_coverage_provenance_and_equal_iteration_weight(self):
        # Keep a source-defined cell, including every one of its required metrics.
        declaration = copy.deepcopy(self.declaration)
        cell = next(s for s in declaration['configurations'] if s['entry'] == 'throughput-cpu-intensive'
                    and s['config']['size'] == 1 and s['config']['clusterSize'] == 5)
        declaration['configurations'] = [cell]
        records = []
        for index in range(5):
            run = suite_record(declaration, f'synthetic-{index}')
            its = run['results'][0]['results'][0]['iteration_results']
            # Median of per-iteration medians = 3; pooled request median = 2.5.
            for it, values in zip(its, [[1] * 10, [2] * 5 + [100] * 5, [3] * 10]):
                it['duration_samples']['rust-agent-invocation'] = values
            records.append((dict(sha256=str(index) * 64), run))
        report = r3.analyze(records, declaration)
        self.assertFalse(report['r3_acceptance'])
        self.assertFalse(report['n_accepted'])
        bands = {b['metric']: b for b in report['bands']}
        local = bands['rust-agent-rpc-local-invocation']
        self.assertEqual(local['exposed_processes'], 5)
        self.assertEqual(local['status'], 'within-noise-threshold')
        self.assertEqual(bands['rust-agent-rpc-remote-invocation']['exposed_processes'], 0)
        self.assertEqual(bands['rust-agent-rpc-remote-invocation']['status'], 'insufficient-coverage')
        self.assertEqual(list(bands['rust-agent-invocation']['process_estimates'].values()), [3] * 5)
        # Artifact divergence must split rather than inflate replication.
        records[-1][1]['artifacts']['runner']['sha256'] = 'e' * 64
        report = r3.analyze(records, declaration)
        locals = [b for b in report['bands'] if b['metric'] == 'rust-agent-rpc-local-invocation']
        self.assertEqual(sorted(b['exposed_processes'] for b in locals), [1, 4])
        self.assertTrue(all(b['status'] == 'insufficient-coverage' for b in locals))
        # Missing mandatory keys in every process cannot disappear from the inventory.
        for _, run in records:
            for it in run['results'][0]['results'][0]['iteration_results']:
                del it['count_samples']['ts-agent-throughput-ops-per-sec']
        report = r3.analyze(records, declaration)
        self.assertTrue(all(b['status'] == 'invalid' for b in report['bands']))

    def test_sparse_union_and_conditional_masks_survive_process_aggregation(self):
        declaration = copy.deepcopy(self.declaration)
        declaration['configurations'] = [next(s for s in declaration['configurations']
                                             if s['entry'] == 'streaming-rpc-reconnect')]
        key = 'storage-completion-keyvalue-get-stream_session_index-read_recovery-page'
        records = []
        for index in range(5):
            run = suite_record(declaration, f'synthetic-{index}')
            if index == 0:
                for it in run['results'][0]['results'][0]['iteration_results']:
                    it['count_samples'][key] = [2]
            records.append(({}, run))
        report = r3.analyze(records, declaration)
        band = next(b for b in report['bands'] if b['metric'] == key)
        self.assertEqual(list(band['process_estimates'].values()), [2, 0, 0, 0, 0])
        self.assertEqual(band['status'], 'noisy')
        self.assertEqual(report['processes'][1]['configurations'][0]['metrics'][key]['observations'][0]['state'],
                         'producer-zero')
        cell = next(s for s in self.declaration['configurations'] if s['entry'] == 'throughput-cpu-intensive'
                    and s['config']['size'] == 1 and s['config']['clusterSize'] == 5)
        declaration['configurations'] = [cell]
        run = suite_record(declaration)
        its = run['results'][0]['results'][0]['iteration_results']
        its[1] = iteration(cell, 1, route='remote')
        local = r3.analyze([({}, run)], declaration)['processes'][0]['configurations'][0]['metrics']['rust-agent-rpc-local-invocation']
        self.assertEqual(local['requested_indexes'], [0, 1, 2])
        self.assertEqual(local['applicable_indexes'], [0, 2])
        self.assertEqual(local['estimate'], 10)
        self.assertEqual(local['observations'][1]['state'], 'not-applicable')
        for index in (0, 2):
            for key in ('rust-agent-rpc-local-invocation', 'rust-agent-rpc-local-worker-0__secondary'):
                its[index]['duration_samples'][key] = [1e308] * 10
        report = r3.analyze([({}, run)], declaration)
        local = report['processes'][0]['configurations'][0]['metrics']['rust-agent-rpc-local-invocation']
        self.assertEqual(local['state'], 'measured')
        self.assertEqual(local['estimate'], 1e308)
        json.dumps(report, allow_nan=False)

    def test_record_identity_and_original_hash_are_required(self):
        declaration = copy.deepcopy(self.declaration)
        declaration['configurations'] = [declaration['configurations'][0]]
        clean = suite_record(declaration)
        for change in ('source', 'hardware', 'artifact', 'missing-iteration', 'missing-config',
                       'extra-entry', 'boolean-version', 'workload-version', 'failure'):
            run = copy.deepcopy(clean)
            if change == 'source':
                run['source']['commitSha'] = 'e' * 40
            elif change == 'hardware':
                run['results'][0]['measurement_environment']['hardware_fingerprint'] = None
            elif change == 'artifact':
                run['artifacts']['services'].pop('golem-worker-executor')
            elif change == 'missing-iteration':
                run['results'][0]['results'][0]['iteration_results'].pop(0)
            elif change == 'missing-config':
                run['results'][0]['results'] = []
            elif change == 'extra-entry':
                extra = copy.deepcopy(run['results'][0])
                extra.update(name='unknown-empty-entry', results=[])
                run['results'].append(extra)
            elif change == 'boolean-version':
                run['harness_version'] = True
            elif change == 'workload-version':
                run['results'][0]['entry_version'] = 2
            else:
                run['results'][0]['results'][0]['failures'] = {'setup': ['failed']}
            with self.subTest(change=change):
                self.assertEqual(len(r3.analyze([({}, run)], declaration)['invalid_inputs']), 1)
        self.assertEqual(len(r3.analyze([({}, clean), ({}, clean)], declaration)['invalid_inputs']), 1)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'record.json'
            path.write_text(json.dumps(dict(runs=[clean])))
            sha = hashlib.sha256(path.read_bytes()).hexdigest()
            self.assertEqual(r3.read_record(path, sha)[1], clean)
            with self.assertRaisesRegex(ValueError, 'digest mismatch'):
                r3.read_record(path, '0' * 64)
            path.write_text('{"runs":[],"runs":[]}')
            with self.assertRaisesRegex(ValueError, 'Duplicate JSON key'):
                r3.read_record(path, hashlib.sha256(path.read_bytes()).hexdigest())

    def test_cli_retains_separate_report_and_never_overwrites(self):
        run = suite_record(self.declaration)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'record.json'
            path.write_text(json.dumps(dict(runs=[run])))
            sha = hashlib.sha256(path.read_bytes()).hexdigest()
            output = Path(directory) / 'view'
            command = ['python3', str(SCRIPT), '--record', str(path), sha,
                       '--producer-root', str(self.producer_root), '--output', str(output)]
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
            result = subprocess.run(command, capture_output=True, text=True, env=env, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads((output / 'report.json').read_text())
            self.assertFalse(report['r3_acceptance'])
            self.assertEqual(report['summary']['invalid_iterations'], 0)
            manifest_hash = hashlib.sha256(r3.canonical(json.loads((output / 'manifest.json').read_text())).encode()).hexdigest()
            self.assertEqual(report['manifest_sha256'], manifest_hash)
            original = (output / 'report.json').read_bytes()
            result = subprocess.run(command, capture_output=True, text=True, env=env, timeout=30)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual((output / 'report.json').read_bytes(), original)
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), sha)
            run['results'][0]['results'][0]['iteration_results'][0]['duration_samples']['invocation'][0] = 10 ** 400
            path.write_text(json.dumps(dict(runs=[run])))
            command[4] = hashlib.sha256(path.read_bytes()).hexdigest()
            command[-1] = str(Path(directory) / 'invalid-view')
            result = subprocess.run(command, capture_output=True, text=True, env=env, timeout=30)
            self.assertEqual(result.returncode, 2, result.stderr)
            invalid = json.loads((Path(command[-1]) / 'report.json').read_text())
            self.assertEqual(invalid['summary']['invalid_iterations'], 1)
            self.assertFalse(invalid['r3_acceptance'])
            with tempfile.TemporaryDirectory() as changed:
                producer = Path(changed) / 'integration-tests/benchmark_suites/ci.yaml'
                producer.parent.mkdir(parents=True)
                producer.write_text('changed producer')
                command[6] = changed
                command[-1] = str(Path(directory) / 'changed-producer')
                result = subprocess.run(command, capture_output=True, text=True, env=env, timeout=30)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('Producer contract changed', result.stderr)
                self.assertFalse(Path(command[-1]).exists())

    def test_coverage_pilot_cli_distinguishes_absence_from_loss(self):
        declaration = copy.deepcopy(self.declaration)
        declaration.update(suite_name='R3 coverage pilot', fixtures=['benchmark_agent_rust_release', 'benchmark_agent_ts'])
        cases = (('throughput-cpu-intensive', 1, 100), ('throughput-cpu-intensive', 10, 100),
                 ('throughput-large-input', 1, 100), ('throughput-large-input', 1, 10000))
        declaration['configurations'] = [copy.deepcopy(next(s for s in declaration['configurations']
            if s['entry'] == name and s['config']['clusterSize'] == 5 and s['config']['size'] == size
            and s['config']['length'] == length)) for name, size, length in cases]
        suite = dict(name='R3 coverage pilot', benchmarks=[
            dict(name='throughput-cpu-intensive', version=1, iterations=5, clusterSize=[5], size=[1, 10], length=[100]),
            dict(name='throughput-large-input', version=1, iterations=5, clusterSize=[5], size=[1], length=[100, 10000])])
        declaration['suite_fingerprint'] = hashlib.sha256(json.dumps(suite, separators=(',', ':')).encode()).hexdigest()
        for item in declaration['configurations']:
            item['iterations'] = 5
        clean = suite_record(declaration)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'raw.json'
            command = ['python3', str(SCRIPT.with_name('r3_coverage.py')), 'analyze', str(path),
                       '--producer-root', str(self.producer_root)]
            for scenario in ('absent', 'exposed', 'lost-sample', 'wrong-suite', 'missing-iteration'):
                run = copy.deepcopy(clean)
                if scenario == 'exposed':
                    for entry in run['results']:
                        for result in entry['results']:
                            item = next(s for s in declaration['configurations'] if s['entry'] == entry['name']
                                        and s['config'] == result['run_config'])
                            result['iteration_results'][1] = iteration(item, 1, route='remote')
                elif scenario == 'lost-sample':
                    run['results'][0]['results'][0]['iteration_results'][0]['duration_samples']['rust-agent-rpc-local-invocation'].pop()
                elif scenario == 'wrong-suite':
                    run['suite_fingerprint'] = r3.SUITE
                elif scenario == 'missing-iteration':
                    run['results'][0]['results'][0]['iteration_results'].pop()
                path.write_text(json.dumps(dict(runs=[run])))
                result = subprocess.run(command, capture_output=True, text=True, timeout=30,
                                        env=dict(os.environ, PYTHONDONTWRITEBYTECODE='1'))
                with self.subTest(scenario=scenario):
                    if scenario in ('absent', 'exposed'):
                        self.assertEqual(result.returncode, 0, result.stderr)
                        report = json.loads(result.stdout)
                        self.assertEqual(len(report['rows']), 16)
                        self.assertEqual(report['coverage_complete'], scenario == 'exposed')
                        self.assertFalse(report['r3_acceptance'])
                        self.assertFalse(report['n_accepted'])
                    else:
                        self.assertNotEqual(result.returncode, 0)

    def test_duration_noise_is_scale_invariant(self):
        declaration = copy.deepcopy(self.declaration)
        declaration['configurations'] = [declaration['configurations'][0]]
        for scale in (1.0, 5e-324, 5e307):
            for multiplier, expected_cv, expected_status in (
                    (1, 0.0, 'within-noise-threshold'),
                    (2, math.sqrt(5) / 6, 'noisy')):
                with self.subTest(scale=scale, multiplier=multiplier):
                    records = []
                    for index in range(5):
                        run = suite_record(declaration, f'scaled-{index}')
                        value = scale * (multiplier if index == 4 else 1)
                        for it in run['results'][0]['results'][0]['iteration_results']:
                            it['duration_samples']['invocation'] = [value]
                        records.append(({}, run))
                    report = r3.analyze(records, declaration)
                    json.dumps(report, allow_nan=False)
                    self.assertEqual(report['summary']['invalid_iterations'], 0)
                    band = next(b for b in report['bands'] if b['metric'] == 'invocation')
                    self.assertEqual(band['exposed_processes'], 5)
                    self.assertEqual(band['status'], expected_status)
                    self.assertAlmostEqual(band['sample_cv'], expected_cv)
                    self.assertEqual(band['max_min'], multiplier)
                    self.assertEqual(band['comparable_control_within_threshold'], multiplier == 1)
                    self.assertFalse(report['r3_acceptance'])
                    self.assertFalse(report['n_accepted'])

    def test_finite_duration_range_retains_report_without_capping_ratio(self):
        with tempfile.TemporaryDirectory() as directory:
            command = ['python3', str(SCRIPT), '--producer-root', str(self.producer_root)]
            for index in range(5):
                run = suite_record(self.declaration, f'finite-range-{index}')
                for it in run['results'][0]['results'][0]['iteration_results']:
                    it['duration_samples']['invocation'] = [1e-308 if index == 0 else 1e308]
                path = Path(directory) / f'record-{index}.json'
                path.write_text(json.dumps(dict(runs=[run]), allow_nan=False))
                command.extend(['--record', str(path), hashlib.sha256(path.read_bytes()).hexdigest()])
            output = Path(directory) / 'view'
            command.extend(['--output', str(output)])
            result = subprocess.run(command, capture_output=True, text=True,
                                    env=dict(os.environ, PYTHONDONTWRITEBYTECODE='1'), timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads((output / 'report.json').read_text())
            self.assertEqual(report['summary']['invalid_iterations'], 0)
            band = next(b for b in report['bands'] if b['entry'] == 'cold-start-unknown-small-no-cache'
                        and b['config']['size'] == 1 and b['metric'] == 'invocation')
            self.assertEqual(band['status'], 'noisy')
            self.assertIsNone(band['max_min'])
            self.assertEqual(band['max_min_status'], 'unrepresentable')
            self.assertFalse(band['comparable_control_within_threshold'])
            self.assertFalse(report['r3_acceptance'])


class SelectorValidationTest(unittest.TestCase):
    source = 'f' * 40

    def record(self):
        declared = selector.declaration(self.source)
        run = suite_record(declared)
        for spec in declared['configurations']:
            raw = next(result for entry in run['results'] if entry['name'] == spec['entry']
                       for result in entry['results'] if result['run_config'] == spec['config'])
            for it in raw['iteration_results']:
                # Independent oracle: even iterations start local; each adjacent target flips.
                start = 'local' if it['index'] in (0, 2, 4) else 'remote'
                for language in ('rust', 'ts'):
                    for route in ('local', 'remote'):
                        prefix = f'{language}-agent-rpc-{route}-'
                        for key in list(it['duration_samples']):
                            if key.startswith(prefix):
                                del it['duration_samples'][key]
                        for key in list(it['count_samples']):
                            if key.startswith(prefix):
                                del it['count_samples'][key]
                    calls = {'throughput-cpu-intensive': 10, 'throughput-large-input': 100}[spec['entry']]
                    for target in range(spec['config']['size']):
                        route = start if target % 2 == 0 else ('remote' if start == 'local' else 'local')
                        prefix = f'{language}-agent-rpc-{route}-'
                        it['duration_samples'][prefix + f'worker-{target}__secondary'] = [10 + target] * calls
                        it['duration_samples'].setdefault(prefix + 'invocation', []).extend([10 + target] * calls)
                        for suffix in ('retries', 'timeouts'):
                            it['count_samples'][prefix + f'worker-{target}-{suffix}__secondary'] = [0] * calls
                            it['count_samples'].setdefault(prefix + 'invocation-' + suffix, []).extend([0] * calls)
        return run

    def test_public_cli_exact_schedule_and_wrong_parity(self):
        run = self.record()
        with tempfile.TemporaryDirectory() as directory:
            raw = Path(directory) / 'raw.json'
            command = ['python3', str(SCRIPT.with_name('r3_selector.py')), 'analyze', str(raw), '--source', self.source]
            for change in ('none', 'parity', 'source', 'version', 'iteration', 'failure', 'retry'):
                damaged = copy.deepcopy(run)
                it = damaged['results'][0]['results'][0]['iteration_results'][0]
                if change == 'parity':
                    # Both routes remain exposed over the process, but iteration0 is wrong.
                    for field in ('duration_samples', 'count_samples'):
                        it[field] = {key.replace('-rpc-local-', '-rpc-remote-'): value for key, value in it[field].items()}
                elif change == 'source':
                    damaged['source']['commitSha'] = r3.SOURCE
                elif change == 'version':
                    damaged['results'][0]['entry_version'] = 1
                elif change == 'iteration':
                    damaged['results'][0]['results'][0]['iteration_results'].pop()
                elif change == 'failure':
                    it['failures'] = {'setup': ['failed']}
                elif change == 'retry':
                    it['count_samples']['rust-agent-rpc-local-invocation-retries'][0] = 1
                raw.write_text(json.dumps(dict(runs=[damaged])))
                result = subprocess.run(command, capture_output=True, text=True, timeout=30,
                                        env=dict(os.environ, PYTHONDONTWRITEBYTECODE='1'))
                with self.subTest(change=change):
                    if change == 'none':
                        self.assertEqual(result.returncode, 0, result.stderr)
                        report = json.loads(result.stdout)
                        self.assertEqual(len(report['rows']), 16)
                        self.assertTrue(report['coverage_complete'])
                        self.assertFalse(report['r3_acceptance'])
                        self.assertFalse(report['n_accepted'])
                        self.assertEqual(report['rows'][0]['applicable_indexes'], [0, 2, 4])
                        self.assertEqual(report['rows'][1]['applicable_indexes'], [1, 3])
                        self.assertEqual(report, selector.analyze(run, self.source))
                    else:
                        self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_selection_receipts_are_complete_and_match_pods(self):
        receipts = []
        for index in range(5):
            for component in range(8):
                for target in range(10 if component >= 6 else 1):
                    local = (index + target) % 2 == 0
                    receipts.append(dict(component_id=f'{index}-{component}', iteration=index, target=target,
                        candidate=3, local=local, parent='Caller("key")', child='Callee("key")',
                        parent_pod='127.0.0.1:9100', child_pod=f'127.0.0.1:{9100 if local else 9101}'))
        text = '\n'.join('\x1b[1mrpc_selection\x1b[0m: ' + json.dumps(item) for item in receipts)
        self.assertEqual(selector.selections(text), receipts)
        selector.validate_selections(receipts)
        for change in ('missing', 'duplicate', 'wrong-pod', 'bad-candidate'):
            damaged = copy.deepcopy(receipts)
            if change == 'missing':
                damaged.pop()
            elif change == 'duplicate':
                damaged.append(damaged[0])
            elif change == 'wrong-pod':
                damaged[0]['child_pod'] = 'different'
            else:
                damaged[0]['candidate'] = 10000
            with self.subTest(change=change), self.assertRaises(ValueError):
                selector.validate_selections(damaged)

    def test_overlay_is_bound_and_never_changes_service_bytes(self):
        import io
        provenance = dict(source=self.source, run_id='123', attempt='1', base_source=r3.SOURCE,
                          base_bundle_sha256=selector.original.BUNDLE_SHA, sha256=hashlib.sha256(b'executable').hexdigest())
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'target/benchmarks').mkdir(parents=True)
            (root / 'target/release').mkdir()
            service = root / 'target/release/worker-executor'
            service.write_bytes(b'original executor')
            for change in ('none', 'source', 'run', 'attempt', 'digest', 'permission', 'extra', 'link', 'duplicate', 'traversal'):
                metadata = copy.deepcopy(provenance)
                if change in ('source', 'run', 'attempt'):
                    metadata[{'source': 'source', 'run': 'run_id', 'attempt': 'attempt'}[change]] = 'wrong'
                if change == 'digest':
                    metadata['sha256'] = '0' * 64
                archive = root / 'overlay.tar.gz'
                with tarfile.open(archive, 'w:gz') as tar:
                    for name, data in (('benchmarks', b'executable'), ('harness-provenance.json', json.dumps(metadata).encode())):
                        member = tarfile.TarInfo(name)
                        member.size = len(data)
                        member.mode = 0o644 if change == 'permission' else 0o755
                        tar.addfile(member, io.BytesIO(data))
                    if change in ('extra', 'link', 'duplicate', 'traversal'):
                        member = tarfile.TarInfo({'extra': 'worker-executor', 'link': 'link',
                                                 'duplicate': 'benchmarks', 'traversal': '../benchmarks'}[change])
                        if change == 'link':
                            member.type = tarfile.SYMTYPE
                            member.linkname = 'benchmarks'
                        tar.addfile(member)
                with self.subTest(change=change):
                    if change == 'none':
                        selector.install_overlay(archive, root, {}, self.source, '123')
                        self.assertEqual((root / 'target/benchmarks/benchmarks').read_bytes(), b'executable')
                    else:
                        with self.assertRaises(ValueError):
                            selector.install_overlay(archive, root, {}, self.source, '123')
                    self.assertEqual(service.read_bytes(), b'original executor')

    def test_original_origin_and_expired_preparation_fail_closed(self):
        origin = dict(id=44078542455, head_sha=r3.SOURCE, path='.github/workflows/benchmark.yaml',
                      event='workflow_dispatch', run_attempt=1, conclusion='success')
        selector.validate_origin(origin)
        for key, value in (('path', '.github/workflows/other.yaml'), ('run_attempt', 2), ('conclusion', 'failure')):
            with self.assertRaises(ValueError):
                selector.validate_origin(dict(origin, **{key: value}))
        with patch.object(selector.subprocess, 'check_output') as process:
            with self.assertRaisesRegex(ValueError, 'reserve'):
                selector.checked(['must-not-run'], ROOT, 0)
            process.assert_not_called()


def campaign_hardware(memory=114 * 1024 ** 3):
    return dict(architecture='x86_64', memory_bytes=memory,
                cpus=[dict(vendor='AuthenticAMD', brand='AMD EPYC', family='25', model='17', stepping='1')
                      for _ in range(32)])


def frozen_budget(prepared):
    (prepared / campaign.HISTORY.name).write_bytes(campaign.HISTORY.read_bytes())
    return dict(budget_policy=campaign.BUDGET_POLICY, prior_budget=campaign.prior_budget())


def campaign_record(declared, run_id, hardware=None):
    run = suite_record(declared, run_id)
    run['artifacts']['runner']['sha256'] = campaign.HARNESS_SHA
    fingerprint = campaign.observed_fingerprint(hardware or campaign_hardware())
    for entry in run['results']:
        entry['measurement_environment']['hardware_fingerprint'] = fingerprint
        entry['series_key'] = r3.digest(json.dumps([1, entry['entry_version'], r3.RUNNER,
            fingerprint, r3.STORAGE, False], separators=(',', ':')).encode())
        if entry['name'] not in r3.CALLS:
            continue
        for result in entry['results']:
            config = result['run_config']
            for it in result['iteration_results']:
                for field in ('duration_samples', 'count_samples'):
                    it[field] = {k: v for k, v in it[field].items()
                                 if '-agent-rpc-local-' not in k and '-agent-rpc-remote-' not in k}
                for language in ('rust', 'ts'):
                    for target in range(config['size']):
                        route = 'local' if config['clusterSize'] == 1 or it['index'] % 2 == target % 2 else 'remote'
                        prefix = f'{language}-agent-rpc-{route}-'
                        samples = [10] * r3.CALLS[entry['name']]
                        it['duration_samples'][prefix + f'worker-{target}__secondary'] = samples
                        it['duration_samples'].setdefault(prefix + 'invocation', []).extend(samples)
                        for suffix in ('retries', 'timeouts'):
                            it['count_samples'][prefix + f'worker-{target}-{suffix}__secondary'] = [0] * len(samples)
                            it['count_samples'].setdefault(prefix + 'invocation-' + suffix, []).extend([0] * len(samples))
    return run


def campaign_selections(declared):
    rows = []
    for config_index, spec in enumerate(declared['configurations']):
        if spec['entry'].removesuffix('-aggregate') not in r3.CALLS:
            continue
        c = spec['config']
        for index in range(5):
            for language in ('rust', 'ts'):
                for target in range(c['size']):
                    local = c['clusterSize'] == 1 or index % 2 == target % 2
                    identity = f'("rpc-test-{target}-3")'
                    rows.append(dict(component_id=f'{config_index}/{index}/{language}', iteration=index,
                        target=target, candidate=3, local=local, parent_pod='p1', child_pod='p1' if local else 'p2',
                        parent=('RustRpcBenchmarkAgent' if language == 'rust' else 'RpcBenchmarkAgent') + identity,
                        child=('RustBenchmarkAgent' if language == 'rust' else 'BenchmarkAgent') + identity,
                        context=[spec['entry'], c['clusterSize'], c['size'], c['length']]))
    return rows


def selection_log(rows):
    return '\n\n'.join('INFO rpc_selection: ' + json.dumps({k: v for k, v in row.items() if k != 'context'})
        + '\n in benchmark with name: "%s", cluster_size: %d, size: %d, length: %d' % tuple(row['context']) for row in rows)


class R3CampaignTest(unittest.TestCase):
    def test_hardware_memory_boundaries_and_cpu_identity(self):
        reference = 114 * 1024 ** 3
        base = campaign.hardware_class(campaign_hardware())
        for memory in (115 * 1024 ** 3, (reference * 98 + 99) // 100, reference * 102 // 100):
            self.assertEqual(campaign.hardware_class(campaign_hardware(memory)), base)
        for memory in ((reference * 98 + 99) // 100 - 1, reference * 102 // 100 + 1):
            with self.assertRaisesRegex(ValueError, 'memory'):
                campaign.hardware_class(campaign_hardware(memory))
        for change in ('architecture', 'count', 'vendor', 'brand', 'missing-model', 'heterogeneous'):
            observation = campaign_hardware()
            if change == 'architecture':
                observation['architecture'] = 'aarch64'
            elif change == 'count':
                observation['cpus'].pop()
            elif change == 'missing-model':
                observation['cpus'][0]['model'] = ''
            elif change == 'heterogeneous':
                observation['cpus'][0]['model'] = '18'
            else:
                observation['cpus'][0][change] = 'other'
            with self.subTest(change=change), self.assertRaises(ValueError):
                campaign.hardware_class(observation)
        different = campaign_hardware()
        for cpu in different['cpus']:
            cpu['model'] = '18'
        self.assertNotEqual(campaign.hardware_class(different), base)
        cpuinfo = '\n\n'.join(f'processor : {i}\nvendor_id : AuthenticAMD\nmodel name : AMD EPYC\n'
                             'cpu family : 25\nmodel : 17\nstepping : 1' for i in range(32))
        self.assertEqual(campaign.hardware_observation('x86_64', cpuinfo, f'MemTotal: {reference // 1024} kB\n'),
                         campaign_hardware())

    def test_class_comparison_preserves_raw_identity_and_other_boundaries(self):
        declared = campaign.declaration(controls=True)
        records, classes = [], {}
        for i in range(5):
            hardware = campaign_hardware((114 + i % 2) * 1024 ** 3)
            run = campaign_record(declared, f'host{i}', hardware)
            records.append((dict(path=f'host{i}'), run))
            classes[run['run_id']] = campaign.hardware_class(hardware)
        saved = copy.deepcopy(records)
        exact = r3.analyze(records, declared)
        self.assertTrue(all(b['exposed_processes'] < 5 for b in exact['bands'] if 'exposed_processes' in b))
        grouped = r3.analyze(records, declared, hardware_classes=classes)
        self.assertEqual(records, saved)
        self.assertTrue(all(len(b['process_estimates']) == 5 for b in grouped['bands']))
        self.assertEqual(set(grouped['summary']['band_statuses']), {'within-noise-threshold', 'not-applicable'})
        self.assertEqual(grouped['processes'][1]['configurations'][0]['identity']['series'],
                         saved[1][1]['results'][0]['series_key'])
        self.assertTrue(all(b['identity']['hardware_class'] == classes['host0'] for b in grouped['bands']))
        records[0][1]['artifacts']['services']['golem-worker-executor']['sha256'] = 'e' * 64
        split = r3.analyze(records, declared, hardware_classes=classes)
        self.assertTrue(all(len(b['process_estimates']) < 5 for b in split['bands']))
        records[0][1]['results'][0]['series_key'] = '0' * 64
        invalid = r3.analyze(records, declared, hardware_classes=classes)
        self.assertEqual(len(invalid['invalid_inputs']), 1)

    def test_ineligible_host_starts_no_process(self):
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = SimpleNamespace(output=root / 'output', prepared=root, root=root, wave=1, partition='bulk')
            with patch.object(campaign, 'bound_campaign', return_value=dict(created_epoch=time.time() - 1200)), \
                 patch.object(campaign, 'hardware_observation', return_value=campaign_hardware(120 * 1024 ** 3)), \
                 patch.object(campaign.subprocess, 'check_output') as process:
                with self.assertRaisesRegex(ValueError, 'memory'):
                    campaign.run(args)
                process.assert_not_called()
            self.assertFalse(json.loads((args.output / 'status.json').read_text())['valid'])
            self.assertFalse((args.output / 'pre').exists())

    def test_preflight_member_names_match_the_installer(self):
        import io
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prepared = root / 'prepared'
            prepared.mkdir()
            for name in campaign.INPUTS:
                (prepared / name).write_bytes(b'unused synthetic preparation receipt')
            archive = prepared / 'benchmark-bundle.tar.gz'
            with tarfile.open(archive, 'w:gz') as tar:
                for name in ('./target/benchmarks/benchmarks', './target/release/worker-executor'):
                    item = tarfile.TarInfo(name)
                    item.size, item.mode = 7, 0o755
                    tar.addfile(item, io.BytesIO(b'payload'))
            now = int(time.time())
            origin = dict(id=campaign.PRODUCER, head_sha=campaign.HARNESS, run_attempt=1,
                path='.github/workflows/benchmark.yaml', event='workflow_dispatch', conclusion='success',
                repository=dict(full_name='golemcloud/golem'), created_at=campaign.datetime.fromtimestamp(now, campaign.timezone.utc).isoformat())
            artifact = dict(id=campaign.ARTIFACT, name='selector-preparation-attempt1', size_in_bytes=357345303,
                expired=False, workflow_run=dict(id=campaign.PRODUCER, head_sha=campaign.HARNESS))
            campaign.save(root / 'origin.json', origin)
            campaign.save(root / 'artifact.json', artifact)
            output = root / 'frozen'
            args = SimpleNamespace(output=output, origin=root / 'origin.json', campaign_origin=root / 'origin.json',
                artifact=root / 'artifact.json', prepared=prepared, deadline=now+36000)
            rejected = SimpleNamespace(**vars(args))
            rejected.output, rejected.prepared = root / 'over-budget', root / 'missing-inputs'
            prior = dict(campaign.prior_budget(), compute_millidollars=6441)
            with patch.object(campaign, 'prior_budget', return_value=prior), \
                 patch.dict(os.environ, GITHUB_ACTIONS='false'):
                with self.assertRaisesRegex(ValueError, 'Cumulative allocation'):
                    campaign.preflight(rejected)
            self.assertEqual(list(rejected.output.iterdir()), [])
            # This test exercises archive member identity, not the independently pinned byte receipt.
            with patch.object(campaign, 'prepared_identity', return_value={}), patch.dict(os.environ, GITHUB_ACTIONS='false'):
                campaign.preflight(args)
            frozen = json.loads((output / 'campaign.json').read_text())['original_members']
            installed = root / 'installed'
            installed.mkdir()
            with patch.object(campaign.original, 'BUNDLE_SHA', campaign.sha(archive)):
                actual = campaign.original.install(archive, installed)
            self.assertEqual(set(frozen), {'target/benchmarks/benchmarks', 'target/release/worker-executor'})
            self.assertEqual(frozen, actual)
            allocation = json.loads((output / 'campaign.json').read_text())['allocation']
            self.assertEqual(allocation['projected_with_remaining_and_storage_dollars'], 147.30)
            self.assertEqual(allocation['contingency_dollars'], 2.70)
            self.assertEqual(campaign.bound_campaign(output)['prior_budget']['compute_millidollars'], 3740)
            # Even a valid JSON receipt with altered charges is not the reviewed frozen history.
            history_path = output / campaign.HISTORY.name
            history = json.loads(history_path.read_text())
            history['runs'][1]['jobs'].pop()
            history_path.write_text(json.dumps(history))
            with self.assertRaisesRegex(ValueError, 'budget differs'):
                campaign.bound_campaign(output)

    def test_cumulative_budget_inclusive_boundary_and_prior_receipts(self):
        prior = campaign.prior_budget()
        self.assertEqual(prior['compute_millidollars'], 3740)
        self.assertEqual(prior['artifact_bytes'], 357699242)
        self.assertEqual(prior['run_ids'], [45384583345, 45390639540])
        initial = campaign.budget_projection(prior, 0, 0, 20)
        self.assertEqual(initial['projected_with_remaining_and_storage_dollars'], 147.30)
        self.assertEqual(initial['storage_reserve_dollars'], 5)
        exact = dict(prior, compute_millidollars=6440)
        self.assertEqual(campaign.budget_projection(exact, 0, 0, 20)
                         ['projected_with_remaining_and_storage_dollars'], 150)
        with self.assertRaisesRegex(ValueError, 'Cumulative allocation'):
            campaign.budget_projection(dict(exact, compute_millidollars=6441), 0, 0, 20)
        for mutation in ('negative-time', 'unknown-runner', 'unassigned-executed', 'duplicate-job', 'missing-run'):
            history = json.loads(campaign.HISTORY.read_text())
            if mutation == 'negative-time':
                history['runs'][0]['jobs'][0]['completed_at'] = '2026-11-24T11:28:00Z'
            elif mutation == 'unknown-runner':
                history['runs'][0]['jobs'][0]['labels'] = ['different-runner']
            elif mutation == 'unassigned-executed':
                history['runs'][0]['jobs'][1]['step_count'] = 1
            elif mutation == 'duplicate-job':
                history['runs'][0]['jobs'].append(history['runs'][0]['jobs'][0])
            else:
                history['runs'].pop()
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / 'history.json'
                path.write_text(json.dumps(history))
                with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                    campaign.prior_budget(path)

    def test_fixed_inventory_versions_and_all_selector_modes(self):
        declared = campaign.declaration()
        self.assertEqual(len(declared['configurations']), 84)
        self.assertEqual([len(campaign.declaration(p)['configurations']) for p in campaign.PARTITIONS], [22, 11, 37, 14])
        expected = {(name + ('-no-cache' if cache else ''), cluster, size, length, cache)
                    for name, _, clusters, sizes, lengths, cache in r3.SUITE_ENTRIES
                    for cluster in clusters for size in sizes for length in lengths}
        actual = {(s['entry'], s['config']['clusterSize'], s['config']['size'], s['config']['length'],
                   s['config']['disableCompilationCache']) for s in declared['configurations']}
        self.assertEqual(actual, expected)
        self.assertEqual(len([b for b in campaign.suite()['benchmarks'] if b['version'] == 2]), 6)
        self.assertTrue(all(s['iterations'] == 5 for s in declared['configurations']))
        rows = campaign_selections(declared)
        self.assertEqual(campaign.validate_selections(campaign.selection_rows(selection_log(rows)), declared),
                         dict(groups=440, pairs=7540))
        odd = [r for r in rows if r['context'] == ['throughput-large-input-aggregate', 5, 25, 100]
               and r['component_id'].endswith('/rust') and r['iteration'] == 0]
        self.assertEqual(sum(r['local'] for r in odd), 13)
        for mutation in ('missing', 'duplicate', 'wrong-route', 'wrong-context', 'wrong-identity'):
            changed = copy.deepcopy(rows)
            if mutation == 'missing':
                changed.pop()
            elif mutation == 'duplicate':
                changed.append(changed[0])
            elif mutation == 'wrong-route':
                changed[0]['local'] = False
            elif mutation == 'wrong-context':
                changed[0]['context'][2] = 999
            else:
                changed[0]['candidate'] = 4
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                campaign.validate_selections(changed, declared)

    def test_controls_overlap_zero_and_parallel_disagreement(self):
        declared = campaign.declaration(controls=True)
        records = [(dict(path=f'host{i}'), campaign_record(declared, f'host{i}')) for i in range(4)]
        analyses = [r3.analyze([record], declared) for record in records]
        windows = [dict(started=i, ended=20 + i) for i in range(4)]
        comparisons = campaign.control_agreement(analyses, windows)
        self.assertTrue(comparisons)
        self.assertTrue(all(item['max_min'] == 1 for item in comparisons))
        with self.assertRaisesRegex(ValueError, 'overlap'):
            campaign.control_agreement(analyses, [dict(started=i * 10, ended=i * 10 + 2) for i in range(4)])
        changed = copy.deepcopy(analyses)
        changed[0]['processes'][0]['configurations'][0]['metrics']['cold-invocation']['estimate'] = 12.01
        with self.assertRaisesRegex(ValueError, 'max/min'):
            campaign.control_agreement(changed, windows)
        changed = copy.deepcopy(analyses)
        changed[0]['processes'][0]['configurations'][0]['metrics']['cold-invocation-retries']['estimate'] = 1
        with self.assertRaises(ValueError):
            campaign.control_agreement(changed, windows)

    def test_dummy_failure_timeout_lateness_and_container_retention(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for kind in ('success', 'failure', 'timeout', 'late', 'container'):
                begin = time.time() - (6 if kind == 'late' else 0)
                command = [sys.executable, '-c', 'import sys,time; ' + {
                    'failure': 'sys.exit(7)', 'timeout': 'time.sleep(30)'
                }.get(kind, 'print("retained dummy output")')]
                with self.subTest(kind=kind), patch.object(campaign.subprocess, 'check_output', return_value='owned-leftover\n' if kind == 'container' else ''):
                    if kind == 'success':
                        campaign.process_run(command, root, root / kind, begin, begin + 30, os.environ)
                    else:
                        with self.assertRaises((ValueError, subprocess.TimeoutExpired)):
                            campaign.process_run(command, root, root / kind, begin,
                                                 begin + (19 if kind == 'timeout' else 30), os.environ)
                    status = json.loads((root / kind / 'status.json').read_text())
                    self.assertEqual(status['valid'], kind == 'success')
                    if kind == 'container':
                        self.assertEqual(status['remaining_containers'], ['owned-leftover'])

    def test_budget_counts_skips_attempts_and_fixed_slots(self):
        now = time.time()
        stamp = lambda seconds: campaign.datetime.fromtimestamp(seconds, campaign.timezone.utc).isoformat()
        frozen = dict(campaign_run_id='123', created_epoch=now - 1200, prior_budget=campaign.prior_budget())
        jobs = dict(jobs=[dict(run_id=123, run_attempt=1, status='completed', conclusion='success',
            labels=['blacksmith-32vcpu-ubuntu-2204'], started_at=stamp(now - 6400), completed_at=stamp(now)) for _ in range(4)]
            + [dict(conclusion='skipped', labels=[])])
        allocation = campaign.budget(frozen, jobs, dict(artifacts=[]), 1)
        self.assertLess(allocation['projected_with_remaining_and_storage_dollars'], 150)
        # At maximum permitted job duration, this new run alone costs $143.56.
        boundary_jobs = copy.deepcopy(jobs)
        for job in boundary_jobs['jobs'][:4]:
            job['started_at'] = stamp(now - 6420)
        frozen['prior_budget'] = dict(frozen['prior_budget'], compute_millidollars=6440)
        self.assertEqual(campaign.budget(frozen, boundary_jobs, dict(artifacts=[]), 1)
                         ['projected_with_remaining_and_storage_dollars'], 150)
        frozen['prior_budget']['compute_millidollars'] += 1
        with self.assertRaisesRegex(ValueError, 'Cumulative allocation'):
            campaign.budget(frozen, boundary_jobs, dict(artifacts=[]), 1)
        frozen['prior_budget'] = campaign.prior_budget()
        self.assertEqual(campaign.wave_start(frozen, 5) - campaign.wave_start(frozen, 1), 4 * 6600)
        for mutation in ('extra-job', 'attempt', 'duration', 'artifact'):
            changed = copy.deepcopy(jobs)
            artifacts = dict(artifacts=[])
            if mutation == 'extra-job':
                changed['jobs'].append(changed['jobs'][0])
            elif mutation == 'attempt':
                changed['jobs'][0]['run_attempt'] = 2
            elif mutation == 'duration':
                changed['jobs'][0]['started_at'] = stamp(now - 6500)
            else:
                artifacts['artifacts'] = [dict(name='r3-wave1-bulk-attempt1', size_in_bytes=campaign.WORKER_BYTES + 1)]
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                campaign.budget(frozen, changed, artifacts, 1)

    def test_public_wave_gate_recomputes_full_evidence_and_rejects_substitution(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prepared, evidence = root / 'prepared', root / 'evidence'
            prepared.mkdir()
            evidence.mkdir()
            start = time.time() - 6300
            original_members = {'target/benchmarks/benchmarks': 'b' * 64}
            original_members.update({'target/release/' + ('worker-executor' if name == 'golem-worker-executor' else name):
                                     'c' * 64 for name in r3.SERVICES})
            original_members.update({f'test-components/{name}.wasm': 'd' * 64 for name in r3.FIXTURES})
            members = dict(original_members, **{'target/benchmarks/benchmarks': campaign.HARNESS_SHA})
            campaign.save(prepared / 'campaign.json', dict(campaign_run_id='123', created_epoch=start - 1200,
                deadline=start + 33000, inputs={}, code={}, original_members=original_members,
                hardware_policy=campaign.HARDWARE_POLICY, **frozen_budget(prepared)))
            for partition in campaign.PARTITIONS:
                host = evidence / f'r3-wave1-{partition}-attempt1'
                host.mkdir()
                campaign.save(host / 'original-members.json', original_members)
                campaign.save(host / 'installed-members.json', members)
                hardware = campaign_hardware((115 if partition == 'streaming' else 114) * 1024 ** 3)
                campaign.save(host / 'hardware.json', hardware)
                campaign.save(host / 'status.json', dict(valid=True, wave=1, partition=partition,
                    started=start + 1, ended=start + 6200))
                for phase, begin in (('pre', 600), ('main', 840), ('post', 5940)):
                    path = host / phase
                    path.mkdir()
                    declared = campaign.declaration(controls=phase != 'main', partition=partition if phase == 'main' else None)
                    record = campaign_record(declared, f'r3-123-w1-{partition}-{phase}', hardware)
                    campaign.save(path / 'raw.json', dict(runs=[record]))
                    (path / 'process.log').write_text(selection_log(campaign_selections(declared)))
                    campaign.save(path / 'status.json', dict(valid=True, exit_code=0, remaining_containers=[],
                        started=start + begin + 1, ended=start + begin + 90, cleanup_ended=start + begin + 91))
            stamp = lambda seconds: campaign.datetime.fromtimestamp(seconds, campaign.timezone.utc).isoformat()
            campaign.save(root / 'jobs.json', dict(jobs=[dict(run_id=123, run_attempt=1, status='completed',
                conclusion='success', labels=['blacksmith-32vcpu-ubuntu-2204'],
                name=f'r3-campaign / wave1 / measure ({partition})',
                started_at=stamp(start), completed_at=stamp(start + 6300)) for partition in campaign.PARTITIONS]))
            campaign.save(root / 'artifacts.json', dict(artifacts=[]))
            command = [sys.executable, str(campaign.HERE / 'r3_campaign.py'), 'gate', '--prepared', str(prepared),
                       '--evidence', str(evidence), '--jobs', str(root / 'jobs.json'),
                       '--artifacts', str(root / 'artifacts.json'), '--wave', '1']
            result = subprocess.run(command + ['--output', str(root / 'valid')], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads((root / 'valid/gate.json').read_text())
            self.assertTrue(report['valid'])
            self.assertFalse(report['r3_acceptance'])
            self.assertFalse(report['n_accepted'])
            raw = evidence / 'r3-wave1-bulk-attempt1/main/raw.json'
            saved = raw.read_text()
            for mutation in ('same-primary-wrong-targets', 'missing-metric', 'source', 'member', 'hardware'):
                data = json.loads(saved)
                if mutation == 'source':
                    data['runs'][0]['source']['commitSha'] = r3.SOURCE
                elif mutation == 'member':
                    data['runs'][0]['artifacts']['services']['golem-worker-executor']['sha256'] = 'e' * 64
                elif mutation == 'hardware':
                    for entry in data['runs'][0]['results']:
                        entry['measurement_environment']['hardware_fingerprint'] = 'e' * 64
                        entry['series_key'] = r3.digest(json.dumps([1, entry['entry_version'], r3.RUNNER,
                            'e' * 64, r3.STORAGE, False], separators=(',', ':')).encode())
                else:
                    entry = next(e for e in data['runs'][0]['results'] if e['name'] == 'throughput-cpu-intensive')
                    config = next(c for c in entry['results'] if c['run_config']['clusterSize'] == 5 and c['run_config']['size'] == 10)
                    samples = config['iteration_results'][0]
                    if mutation == 'missing-metric':
                        samples['count_samples'].pop('rust-agent-rpc-throughput-ops-per-sec')
                    else:
                        # Preserve all primary totals, while assigning target0/1 to the wrong route.
                        for field in ('duration_samples', 'count_samples'):
                            for key in list(samples[field]):
                                if '-rpc-local-worker-0' in key:
                                    samples[field][key.replace('-local-worker-0', '-remote-worker-0')] = samples[field].pop(key)
                                elif '-rpc-remote-worker-1' in key:
                                    samples[field][key.replace('-remote-worker-1', '-local-worker-1')] = samples[field].pop(key)
                raw.write_text(json.dumps(data))
                result = subprocess.run(command + ['--output', str(root / mutation)], capture_output=True, text=True)
                with self.subTest(mutation=mutation):
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(json.loads((root / mutation / 'gate.json').read_text())['valid'])
                raw.write_text(saved)

    def test_public_reports_preserve_failed_and_cancelled_prefixes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            prepared, evidence = root / 'prepared', root / 'evidence'
            prepared.mkdir()
            evidence.mkdir()
            campaign.save(prepared / 'campaign.json', dict(campaign_run_id='123',
                inputs={}, code={}, hardware_policy=campaign.HARDWARE_POLICY, created_epoch=time.time() - 1200,
                **frozen_budget(prepared)))
            jobs = []
            for partition in campaign.PARTITIONS:
                host = evidence / f'r3-wave1-{partition}-attempt1'
                (host / 'pre').mkdir(parents=True)
                campaign.save(host / 'pre/status.json', dict(valid=True, exit_code=0, remaining_containers=[]))
                (host / 'pre/raw.json').write_text('{"retained": "not validated by inventory"}')
                if partition == 'streaming':
                    campaign.save(host / 'status.json', dict(valid=False, error='Wrong hardware class'))
                jobs.append(dict(id=len(jobs), name=f'r3-campaign / wave1 / measure ({partition})',
                    run_id=123, run_attempt=1, status='completed',
                    conclusion='failure' if partition == 'streaming' else 'cancelled'))
            jobs.append(dict(id=100, name='r3-campaign / wave2', run_id=123, run_attempt=1,
                             status='completed', conclusion='skipped'))
            campaign.save(root / 'jobs.json', dict(jobs=jobs))
            campaign.save(root / 'artifacts.json', dict(artifacts=[]))
            for mode in ('gate', 'final'):
                command = [sys.executable, str(campaign.HERE / 'r3_campaign.py'), mode,
                    '--prepared', str(prepared), '--evidence', str(evidence), '--jobs', str(root / 'jobs.json'),
                    '--artifacts', str(root / 'artifacts.json'), '--output', str(root / mode)]
                if mode == 'gate':
                    command += ['--wave', '1']
                result = subprocess.run(command, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                report = json.loads((root / mode / f'{mode}.json').read_text())
                self.assertFalse(any(report[key] for key in ('valid', 'r3_acceptance', 'n_accepted', 'publication')))
                workers = report['diagnostics']['workers']
                self.assertEqual(len(workers), 4 if mode == 'gate' else 20)
                for worker in workers[:4]:
                    self.assertEqual(worker['phases']['pre']['status']['record']['exit_code'], 0)
                    self.assertTrue(worker['phases']['pre']['raw_present'])
                    self.assertEqual(worker['phases']['main']['status']['state'], 'missing')
                    self.assertFalse(worker['phases']['post']['raw_present'])
                self.assertEqual(workers[0]['job']['conclusion'], 'cancelled')
                self.assertEqual(workers[0]['status']['state'], 'missing')
                self.assertEqual(workers[3]['status']['record']['error'], 'Wrong hardware class')
                self.assertEqual(workers[3]['job']['conclusion'], 'failure')
                self.assertIn('hardware.json', report['error'])
                if mode == 'final':
                    self.assertIsNone(workers[4]['job'])
                    self.assertEqual(report['diagnostics']['wave_jobs'][0]['conclusion'], 'skipped')
            # Bad or non-finite status data must not discard the other hosts' diagnostics.
            status = evidence / 'r3-wave1-startup-attempt1/status.json'
            for text in ('{', '[]', '{"ended": NaN}'):
                status.write_text(text)
                diagnostics = campaign.worker_diagnostics(evidence, dict(jobs=jobs), (1,), '123')
                self.assertEqual(diagnostics['workers'][0]['status']['state'], 'unreadable')
                self.assertEqual(diagnostics['workers'][3]['status']['record']['error'], 'Wrong hardware class')
                json.dumps(diagnostics, allow_nan=False)
            with self.assertRaisesRegex(ValueError, 'another run'):
                campaign.worker_diagnostics(evidence, dict(jobs=jobs), (1,), '456')


if __name__ == '__main__':
    unittest.main()
