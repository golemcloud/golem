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
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[4]
SCRIPT = ROOT / 'integration-tests/benchmark_suites/validate_r3_metrics.py'
spec = importlib.util.spec_from_file_location('r3_metrics', SCRIPT)
r3 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r3)
sys.path.insert(0, str(SCRIPT.parent))
import r3_selector as selector


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


if __name__ == '__main__':
    unittest.main()
