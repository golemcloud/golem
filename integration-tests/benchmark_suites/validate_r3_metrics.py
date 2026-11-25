#!/usr/bin/env python3
"""Interpret saved schema-2 CI metrics without measuring, publishing or accepting R3."""
import argparse
import collections
import hashlib
import itertools
import json
import math
from pathlib import Path
import re
import statistics

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = 'r3-metric-contract-v1'
SOURCE = 'eb46c797c329e1787fb9260cc57b132e0830489d'
SUITE = '177ef1689601b625e469e8479effb295585b5ef76917b7b18af52598f7ea0bd3'
RUNNER = 'github-actions-blacksmith-32vcpu'
STORAGE = 'postgres-kv-indexed-scheduler+filesystem-blob+sqlite-shard'
SERVICES = {'golem-component-compilation-service', 'golem-registry-service',
            'golem-shard-manager', 'golem-worker-executor', 'golem-worker-service'}
FIXTURES = {'benchmark_agent_effect', 'benchmark_agent_rust_release', 'benchmark_agent_ts',
            'golem_it_agent_rpc_rust_release', 'golem_it_tool_streaming_rust_caller_release',
            'golem_it_tool_streaming_rust_provider_release'}
CALLS = {'throughput-echo': 250, 'throughput-large-input': 100,
         'throughput-cpu-intensive': 10}
COARSE = ('keyvalue-get', 'keyvalue-get_many', 'keyvalue-set_many',
          'keyvalue-compare_and_set_many', 'indexed-read')
RECOVERY_PHASES = ('startup', 'pre-demand-5s', 'first-resumed-item',
                   'completion', 'post-completion-35s')
# name, partition, cluster sizes, sizes, lengths, compilation-cache disabled.
SUITE_ENTRIES = (
    ('cold-start-unknown-small', 'startup', [1], [1, 5, 10], [2], True),
    ('cold-start-unknown-medium', 'startup', [1], [1, 5, 10], [5], True),
    ('cold-start-unknown-small', 'startup', [1], [1, 5, 10], [2], False),
    ('cold-start-unknown-medium', 'startup', [1], [1, 5, 10], [5], False),
    ('latency-small', 'startup', [1], [100, 250, 500, 1000], [2], False),
    ('latency-medium', 'startup', [1], [100, 250, 500], [5], False),
    ('sleep', 'startup', [1], [10, 100, 500], [10000], False),
    ('durability-overhead', 'durability-echo', [1], [10, 50, 100], [5000], False),
    ('throughput-echo', 'durability-echo', [1], [1, 10, 50, 100], [1000], False),
    ('throughput-echo-aggregate', 'durability-echo', [1], [1, 10, 50, 100], [1000], False),
    ('throughput-large-input', 'bulk', [1, 5], [1, 10, 25], [100, 10000], False),
    ('throughput-large-input-effect', 'bulk', [1], [1], [10000], False),
    ('throughput-large-input-aggregate', 'bulk', [1, 5], [1, 10, 25], [100, 10000], False),
    ('throughput-cpu-intensive', 'bulk', [1, 5], [1, 10, 25], [100], False),
    ('throughput-cpu-intensive-aggregate', 'bulk', [1, 5], [1, 10, 25], [100], False),
    ('streaming-tool', 'streaming', [1], [1, 10], [512], False),
    ('streaming-rpc', 'streaming', [1], [1, 10], [512], False),
    ('streaming-rpc-history', 'streaming', [1], [0, 32, 128], [512], False),
    ('streaming-rpc-cold-indexed', 'streaming', [1], [0, 128], [512], False),
    ('streaming-rpc-reconnect', 'streaming', [1], [129], [64], False),
    ('streaming-rpc-recovery', 'streaming', [1], [0, 129], [64], False),
    ('streaming-rpc-recovery-siblings', 'streaming', [1], [129], [64], False),
    ('streaming-rpc-recovery-nested', 'streaming', [1], [129], [32], False),
)
# These digests bind the interpretation to the reviewed producer, not the analyzer's HEAD.
PRODUCERS = {
    'integration-tests/benchmark_suites/ci.yaml': '85e30814e846831ac515d3bdf7f5d45815f2f47b489ed6cb83f76256f55c6d0b',
    'integration-tests/src/benchmarks/mod.rs': '6c44879309f3b0ed77348108e40007124ce565eb1962addfc81adf38478b4f43',
    'integration-tests/src/benchmarks/cold_start_unknown.rs': '22a977fc74ae3e56423a32a6a25d2d7a2e0fa6a94a599337eb0c5c1cf81b2877',
    'integration-tests/src/benchmarks/latency.rs': '468c6762d8849b5d91a11426a803bcf7e6f41a2c4d367f268d01539305454eb9',
    'integration-tests/src/benchmarks/sleep.rs': '41caa092f09cb97aecb8075ad5b14912a29ea31f698196010ba77daf5da00f53',
    'integration-tests/src/benchmarks/durability_overhead.rs': '2e88af05d34f503aeaae73d83335b6c7686474d1f4a54f353991edc50fdb3add',
    'integration-tests/src/benchmarks/throughput.rs': '5b5ae10eafd5bf6fe8c64eb0ea55cf9ddfcf09f38a7e472564bbcf41bdd7d064',
    'integration-tests/src/benchmarks/throughput_effect.rs': 'b25b943c09f797e1e25bfa3764a21e045e4fc146e7940733049beefc4dada439',
    'integration-tests/src/benchmarks/streaming.rs': '0563942f043ca23dc53d7969c822c1d7fd5e82b878300492d16982322b43b895',
    'integration-tests/src/benchmarks/streaming_history.rs': '5dc82392c4abead687b0ec319d9103231509ef69392d61afb5b0ebfa1e8b8bd4',
    'integration-tests/src/benchmarks/streaming_recovery.rs': '330e4825393167034fd6e436d7de43c7f42a77a482019ef35562e1b8d0852a04',
    'golem-test-framework/src/benchmark/mod.rs': 'f77befa2f7bddc98f0eb320887fb787d14a4dda5ba3fd757b388a8c905166bfa',
    'golem-test-framework/src/benchmark/config.rs': '4a0d5772c864387dd30d04e72f6838e1f4ae76e04f28280654375a74f22f48d1',
    'golem-test-framework/src/benchmark/results.rs': '77d1d50f5087032afbc0f10448ec045658812f6004f83c63d2f2ca6ed95df6ab',
    'golem-test-framework/src/benchmark/storage_metrics.rs': '0e7a93a6191e63cdd4a00f60210bae27705d33a3986aa157c57a8a05bc9958aa',
    'golem-service-base/src/metrics.rs': '37899c0434c260507b1873bce561669b5bf5d113126912acfae5834467602c50',
}


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def median(values):
    ordered = sorted(values)
    middle = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[middle]
    low, high = ordered[middle - 1:middle + 1]
    # Avoid overflowing the sum of two finite nonnegative observations.
    return low + (high - low) / 2


def selected(phase):
    return [f'storage-{phase}-keyvalue-get_many-stream_session_index-read_recovery-{entity}'
            for entity in ('page', 'session')]


def metric_rules(name, config):
    """Declare required observations from benchmark modes, never from observed keys."""
    metrics = {}
    phases = []

    def add(key, quantity, samples=1, role='fixed', **extra):
        metrics[key] = dict(quantity=quantity, samples=samples, role=role, **extra)

    def invocation(prefix, samples):
        add(prefix + 'invocation', 'duration', samples)
        for suffix in ('retries', 'timeouts'):
            add(prefix + 'invocation-' + suffix, 'event-count', samples, zero=True)

    size, length = config['size'], config['length']
    if name.startswith('cold-start-unknown-') or name == 'sleep':
        invocation('', size)
    elif name in ('latency-small', 'latency-medium'):
        invocation('cold-', size)
        invocation('hot-', size * length)
    elif name == 'durability-overhead':
        for prefix in ('durable-persistent-', 'durable-persistent-commit-', 'ephemeral-'):
            invocation(prefix, size)
    elif name == 'throughput-large-input-effect':
        invocation('effect-agent-', size * 100)
    elif name.removesuffix('-aggregate') in CALLS:
        base = name.removesuffix('-aggregate')
        for language in ('rust', 'ts'):
            for transport in ('', 'http-', 'rpc-'):
                prefix = f'{language}-agent-{transport}'
                add(prefix + 'throughput-ops-per-sec', 'rate')
                if name.endswith('-aggregate'):
                    add(prefix + 'batch-duration', 'duration')
                    add(prefix + 'batch-completions', 'event-count', expected=size * CALLS[base])
                elif transport != 'rpc-':
                    invocation(prefix, size * CALLS[base])
                else:
                    for route in ('local', 'remote'):
                        key = prefix + route + '-invocation'
                        extra = dict(role='conditional-rpc', language=language, route=route,
                                     structural_na=config['clusterSize'] == 1 and route == 'remote')
                        add(key, 'duration', None, **extra)
                        for suffix in ('retries', 'timeouts'):
                            add(key + '-' + suffix, 'event-count', None, zero=True, **extra)
    elif name in ('streaming-tool', 'streaming-rpc'):
        invocation('', size)
        add('guest-time-to-first-chunk', 'duration', size)
    elif name in ('streaming-rpc-history', 'streaming-rpc-cold-indexed'):
        for key in (('invocation', 'guest-time-to-first-chunk') if name.endswith('-history')
                    else ('executor-stop', 'executor-grpc-ready', 'executor-routable',
                          'session-accepted', 'result-mapped', 'first-item', 'stream-complete')):
            add(key, 'duration')
        for key, expected in (('seeded-sessions', size), ('stream-items', length), ('stream-terminals', 1)):
            add(key, 'event-count', expected=expected)
        coarse_phases = ('measured',) if name.endswith('-history') else ('startup', 'accepted', 'first', 'completion')
        for phase in coarse_phases:
            for operation in COARSE:
                add(f'storage-{phase}-{operation}', 'event-count')
    elif name in ('streaming-rpc-reconnect', 'streaming-rpc-recovery',
                  'streaming-rpc-recovery-siblings', 'streaming-rpc-recovery-nested'):
        producers = 2 if name == 'streaming-rpc-recovery' else 1
        leaves = 2 if name.endswith(('-siblings', '-nested')) else 1
        for key in ('session-accepted', 'result-mapped', 'first-resumed-item', 'stream-complete'):
            add(key, 'duration', producers)
        for key in ('seeded-sessions', 'output-items', 'output-terminals',
                    'logical-invocation-starts', 'logical-invocation-finishes', 'successful-session-finishes'):
            add(key, 'event-count', producers)
        for producer in range(producers):
            for leaf in range(leaves):
                add(f'first-resumed-item-producer-{producer}-leaf-{leaf}', 'duration')
                add(f'resumed-items-producer-{producer}-leaf-{leaf}', 'event-count')
        if name != 'streaming-rpc-reconnect':
            for key in ('executor-stop', 'executor-grpc-ready', 'executor-routable'):
                add(key, 'duration')
            add('retired-session-records-unchanged', 'event-count', producers)
            phases = list(RECOVERY_PHASES)
        else:
            phases = ['first-resumed-item', 'completion']
        for phase in phases:
            for key in selected(phase):
                add(key, 'event-count', role='selected-storage', phase=phase)
    else:
        raise ValueError(f'Unknown benchmark mode: {name}')
    return dict(metrics=metrics, sparse_phases=phases)


def manifest(producer_root=ROOT):
    for path, expected in PRODUCERS.items():
        require(digest((producer_root / path).read_bytes()) == expected, f'Producer contract changed: {path}')
    suite = dict(name='CI', benchmarks=[])
    for name, partition, clusters, sizes, lengths, cache in SUITE_ENTRIES:
        # BenchmarkSuiteItem's Serialize field order, not YAML insertion order.
        item = dict(name=name, version=1, iterations=3, partition=partition,
                    clusterSize=clusters, size=sizes, length=lengths)
        if cache:
            item['disableCompilationCache'] = True
        suite['benchmarks'].append(item)
    require(digest(json.dumps(suite, separators=(',', ':')).encode()) == SUITE,
            'Suite changed: review and declare a new metric manifest')
    configurations = []
    for entry in suite['benchmarks']:
        cache = entry.get('disableCompilationCache', False)
        name = entry['name'] + ('-no-cache' if cache else '')
        for cluster, size, length in itertools.product(entry['clusterSize'], entry['size'], entry['length']):
            config = dict(clusterSize=cluster, size=size, length=length, disableCompilationCache=cache)
            configurations.append(dict(entry=name, config=config, iterations=entry['iterations'],
                                       version=entry['version'], partition=entry['partition'],
                                       **metric_rules(name, config)))
    return dict(contract=CONTRACT, source=SOURCE, schema_version=2, harness_version=1,
                suite_name='CI', suite_fingerprint=SUITE, fixtures=sorted(FIXTURES), producer_files=PRODUCERS,
                analyzer_sha256=digest(Path(__file__).read_bytes()), configurations=configurations,
                thresholds=dict(exposed_processes=5, sample_cv=0.10, comparable_control_max_min=1.20),
                units=dict(duration='milliseconds', rate='operations/second',
                           **{'event-count': 'logical events; storage is executor-wide API calls'}),
                sparse_family='storage-{required-phase}-{keyvalue|indexed|blob}-{operation}-{service}-{api}-{entity}')


def sparse_phase(key, phases):
    for phase in phases:
        if re.fullmatch(r'storage-' + re.escape(phase)
                        + r'-(keyvalue|indexed|blob)-[a-z0-9_]+-[a-z0-9_]+-[a-z0-9_-]+-[a-z0-9_-]*', key):
            return phase
    return None


def numbers(values, quantity, key):
    require(isinstance(values, list) and bool(values), f'Empty or malformed samples: {key}')
    for value in values:
        if quantity == 'duration':
            require(type(value) in (int, float) and math.isfinite(value) and value >= 0,
                    f'Invalid duration: {key}')
        else:
            require(type(value) is int and 0 <= value < 2 ** 64, f'Invalid count/rate: {key}')


def rpc_exposure(iteration, config, name):
    durations, counts = iteration['duration_samples'], iteration['count_samples']
    exposures = {}
    calls = CALLS[name]
    for language in ('rust', 'ts'):
        workers = set()
        total = 0
        for route in ('local', 'remote'):
            prefix = f'{language}-agent-rpc-{route}-'
            values = durations.get(prefix + 'invocation', [])
            require(not (route == 'remote' and config['clusterSize'] == 1 and values),
                    'Remote RPC exposure on a one-executor cluster')
            route_workers = {}
            worker_pattern = re.compile(re.escape(prefix) + r'worker-(0|[1-9][0-9]*)__secondary')
            for key, samples in durations.items():
                match = worker_pattern.fullmatch(key)
                if match:
                    worker = int(match[1])
                    require(worker not in workers and worker in range(config['size']),
                            f'Duplicate or unknown RPC target: {language}/{worker}')
                    workers.add(worker)
                    require(len(samples) == calls, f'RPC target sample loss: {key}')
                    route_workers[worker] = samples
                    for suffix in ('retries', 'timeouts'):
                        counter = prefix + f'worker-{worker}-{suffix}__secondary'
                        require(len(counts.get(counter, [])) == calls and not any(counts[counter]),
                                f'RPC target retry/timeout accounting: {counter}')
            require(sorted(values) == sorted(itertools.chain.from_iterable(route_workers.values())),
                    f'RPC primary/target disagreement: {prefix}')
            for suffix in ('retries', 'timeouts'):
                key = prefix + 'invocation-' + suffix
                require(len(counts.get(key, [])) == len(values) and not any(counts.get(key, [])),
                        f'RPC retry/timeout accounting: {key}')
            exposures[language + '/' + route] = len(values)
            total += len(values)
        require(workers == set(range(config['size'])) and total == config['size'] * calls,
                f'Incomplete RPC target coverage: {language}')
        require(len(counts.get(f'{language}-agent-rpc-throughput-ops-per-sec', [])) == 1,
                f'Missing overall RPC throughput: {language}')
    return exposures


def iteration_view(iteration, specification, sparse_keys):
    """Return a separate view; an invalid iteration never supplies a derived zero."""
    definitions = dict(specification['metrics'])
    for key in sparse_keys:
        definitions[key] = dict(quantity='event-count', samples=1, role='sparse-storage',
                                phase=sparse_phase(key, specification['sparse_phases']))
    exposures = {}
    try:
        require(iteration.get('skipped') is None and iteration['failures'] == {}, 'Failed or skipped iteration')
        for field, quantity in (('duration_samples', 'duration'), ('count_samples', 'event-count')):
            require(isinstance(iteration[field], dict), f'Malformed {field}')
            for key, values in iteration[field].items():
                require('recovered' not in key, f'Recovered invocation: {key}')
                numbers(values, quantity, key)
                if key.endswith('__secondary'):
                    continue
                require(key in definitions, f'Unknown primary metric family: {key}')
                expected_field = 'duration_samples' if definitions[key]['quantity'] == 'duration' else 'count_samples'
                require(field == expected_field, f'Wrong sample quantity: {key}')
        name = specification['entry']
        if name in CALLS:
            exposures = rpc_exposure(iteration, specification['config'], name)
        for key, definition in definitions.items():
            field = 'duration_samples' if definition['quantity'] == 'duration' else 'count_samples'
            values = iteration[field].get(key)
            if definition['role'] == 'conditional-rpc':
                exposure = exposures[definition['language'] + '/' + definition['route']]
                require(len(values or []) == exposure, f'Routing sample loss: {key}')
                continue
            if definition['role'] == 'sparse-storage' and values is None:
                continue
            require(values is not None and len(values) == definition['samples'], f'Missing required samples: {key}')
            require(not definition.get('zero') or not any(values), f'Retries/timeouts: {key}')
            require('expected' not in definition or values == [definition['expected']],
                    f'Incorrect completion count: {key}')
        observations = {}
        for key, definition in definitions.items():
            field = 'duration_samples' if definition['quantity'] == 'duration' else 'count_samples'
            values = iteration[field].get(key)
            if definition['role'] == 'conditional-rpc':
                exposure = exposures[definition['language'] + '/' + definition['route']]
                if not exposure:
                    observations[key] = dict(state='not-applicable', value=None, exposure=0,
                                             reason='structurally-impossible' if definition['structural_na']
                                             else 'complete-accounting-zero-exposure')
                    continue
            if values is None:
                observations[key] = dict(state='producer-zero', value=0, reason='successful-sparse-phase',
                                         witnesses=selected(definition['phase']))
            else:
                value = median(values) if definition['quantity'] == 'duration' else sum(values)
                require(math.isfinite(value), f'Unrepresentable estimate: {key}')
                observations[key] = dict(state='measured', value=value, samples=len(values))
                if definition['role'] == 'conditional-rpc':
                    observations[key]['exposure'] = exposure
        return dict(index=iteration['index'], valid=True, exposures=exposures, observations=observations)
    except (ValueError, KeyError, TypeError, OverflowError) as error:
        return dict(index=iteration.get('index'), valid=False, reason=str(error), exposures=exposures,
                    observations={key: dict(state='invalid', value=None, reason=str(error)) for key in definitions})


def configuration_key(entry, config):
    return canonical([entry, config])


def validate_identity(run, declaration):
    require(type(run['schema_version']) is int and run['schema_version'] == 2
            and type(run['harness_version']) is int and run['harness_version'] == 1,
            'Unknown raw schema/harness')
    require(run['source']['repository'] == 'golemcloud/golem' and run['source']['commitSha'] == declaration['source'],
            'Unknown producer source')
    require(run['suite_fingerprint'] == declaration['suite_fingerprint']
            and run['suite'] == declaration['suite_name'], 'Unknown suite')
    require(run['runner']['id'] == RUNNER, 'Unknown runner')
    require(isinstance(run['run_id'], str) and bool(run['run_id']), 'Missing process identity')
    artifacts = run['artifacts']
    require(set(artifacts) == {'runner', 'services', 'fixtures'}
            and set(artifacts['services']) == SERVICES and set(artifacts['fixtures']) == set(declaration['fixtures']),
            'Incomplete artifact provenance')
    members = {'runner': artifacts['runner'], **artifacts['services'], **artifacts['fixtures']}
    require(all(re.fullmatch('[0-9a-f]{64}', item['sha256']) for item in members.values()),
            'Missing artifact digest')
    expected = {configuration_key(s['entry'], s['config']): s for s in declaration['configurations']}
    found = {}
    identities = {}
    names = set()
    for entry in run['results']:
        require(entry['name'] not in names, 'Duplicate benchmark entry')
        names.add(entry['name'])
        require(not entry.get('failures'), 'Entry lifecycle failure')
        env = entry['measurement_environment']
        require(env['runner_id'] == RUNNER and env['storage_profile'] == STORAGE
                and env['sockets_enabled'] is False
                and re.fullmatch('[0-9a-f]{64}', env['hardware_fingerprint']), 'Invalid measurement environment')
        series = [1, entry['entry_version'], RUNNER, env['hardware_fingerprint'], STORAGE, False]
        require(entry['series_key'] == digest(json.dumps(series, separators=(',', ':')).encode()),
                'Invalid series identity')
        for result in entry['results']:
            key = configuration_key(entry['name'], result['run_config'])
            require(key in expected and key not in found, f'Missing/duplicate/unknown configuration: {key}')
            specification = expected[key]
            require(entry['iterations'] == specification['iterations'] and entry['entry_version'] == specification['version'],
                    'Unknown iteration count or entry version')
            require(not result.get('failures'), f'Configuration lifecycle failure: {key}')
            require([it['index'] for it in result['iteration_results']] == list(range(entry['iterations']))
                    and all(type(it['index']) is int for it in result['iteration_results']),
                    f'Missing or reordered requested iterations: {key}')
            found[key] = result
            identities[key] = dict(source=run['source']['commitSha'], suite=declaration['suite_fingerprint'],
                                   harness=1, version=entry['entry_version'], series=entry['series_key'],
                                   environment=env, artifact_hashes={k: v['sha256'] for k, v in members.items()})
    require(names == {s['entry'] for s in declaration['configurations']} and set(found) == set(expected),
            'Incomplete source-defined suite/configuration coverage')
    return found, identities


def noise(values, quantity):
    """Thresholds describe comparable process estimates, not raw-request replication."""
    if not values:
        return dict(status='insufficient-coverage', exposed_processes=0)
    require(all(math.isfinite(v) and v >= 0 for v in values), 'Invalid process estimate')
    if quantity != 'event-count' and min(values) == 0:
        return dict(status='under-resolution', exposed_processes=len(values))
    if len(values) < 5:
        return dict(status='insufficient-coverage', exposed_processes=len(values))
    # CV is dimensionless; normalize before aggregation to avoid magnitude-dependent rounding.
    scale = max(values)
    scaled = [value / scale for value in values] if scale else values
    cv = statistics.stdev(scaled) / statistics.mean(scaled) if scale else 0.0
    ratio = max(values) / min(values) if min(values) else (1.0 if max(values) == 0 else None)
    ratio_status = 'zero-denominator' if ratio is None else 'measured'
    if ratio is not None and not math.isfinite(ratio):
        ratio, ratio_status = None, 'unrepresentable'
    return dict(status='within-noise-threshold' if cv <= 0.10 else 'noisy', exposed_processes=len(values),
                sample_cv=cv, max_min=ratio, max_min_status=ratio_status, comparable_control_threshold=1.20,
                comparable_control_within_threshold=ratio is not None and ratio <= 1.20,
                control_qualification='requires separately established comparable parallel controls')


def analyze(records, declaration, hardware_classes=None):
    """The campaign may supply verified comparison classes; raw identities remain unchanged."""
    if hardware_classes is not None:
        require(set(hardware_classes) == {run['run_id'] for _, run in records}
                and all(re.fullmatch('[0-9a-f]{64}', value) for value in hardware_classes.values()),
                'Incomplete hardware comparison classes')
    specifications = {configuration_key(s['entry'], s['config']): s for s in declaration['configurations']}
    accepted, invalid, seen = [], [], set()
    for receipt, run in records:
        try:
            require(run['run_id'] not in seen, 'Duplicate process identity')
            seen.add(run['run_id'])
            configurations, identities = validate_identity(run, declaration)
            accepted.append((receipt, run, configurations, identities))
        except (ValueError, KeyError, TypeError) as error:
            invalid.append(dict(input=receipt, reason=str(error)))
    sparse = collections.defaultdict(set)
    for _, _, configurations, _ in accepted:
        for key, result in configurations.items():
            phases = specifications[key]['sparse_phases']
            for iteration in result['iteration_results']:
                counts = iteration.get('count_samples')
                if isinstance(counts, dict):
                    sparse[key].update(metric for metric in counts
                                       if sparse_phase(metric, phases) and metric not in specifications[key]['metrics'])
    processes, groups = [], collections.defaultdict(list)
    for receipt, run, configurations, identities in accepted:
        process = dict(run_id=run['run_id'], input=receipt, configurations=[])
        for key, result in configurations.items():
            specification = specifications[key]
            views = [iteration_view(it, specification, sparse[key]) for it in result['iteration_results']]
            metrics = {}
            definitions = dict(specification['metrics'])
            definitions.update({k: dict(quantity='event-count', role='sparse-storage') for k in sparse[key]})
            for metric, definition in sorted(definitions.items()):
                observations = [view['observations'][metric] for view in views]
                values = [o['value'] for o in observations if o['state'] in ('measured', 'producer-zero')]
                valid = all(view['valid'] for view in views)
                metrics[metric] = dict(quantity=definition['quantity'], role=definition['role'],
                                       structural_na=definition.get('structural_na', False),
                                       state='invalid' if not valid else ('measured' if values else 'not-applicable'),
                                       estimate=median(values) if values and valid else None,
                                       requested_indexes=[view['index'] for view in views],
                                       applicable_indexes=[view['index'] for view, o in zip(views, observations)
                                                           if o['state'] in ('measured', 'producer-zero')],
                                       observations=observations)
            item = dict(entry=specification['entry'], config=specification['config'],
                        identity=identities[key], valid=all(v['valid'] for v in views),
                        iterations=views, metrics=metrics)
            process['configurations'].append(item)
            comparison = identities[key]
            if hardware_classes is not None:
                comparison = {k: v for k, v in comparison.items() if k not in ('series', 'environment')}
                comparison['environment'] = {k: v for k, v in identities[key]['environment'].items()
                                             if k != 'hardware_fingerprint'}
                comparison['hardware_class'] = hardware_classes[run['run_id']]
            groups[(key, canonical(comparison))].append((run['run_id'], item))
        processes.append(process)
    bands = []
    for (key, identity), members in sorted(groups.items()):
        for metric in sorted(members[0][1]['metrics']):
            definition = members[0][1]['metrics'][metric]
            estimates = {rid: item['metrics'][metric]['estimate'] for rid, item in members}
            values = [value for value in estimates.values() if value is not None]
            if any(item['metrics'][metric]['state'] == 'invalid' for _, item in members):
                evaluation = dict(status='invalid')
            elif definition['structural_na']:
                evaluation = dict(status='not-applicable', reason='structurally-impossible')
            else:
                evaluation = noise(values, definition['quantity'])
            bands.append(dict(entry=specifications[key]['entry'], config=specifications[key]['config'],
                              metric=metric, quantity=definition['quantity'], identity=json.loads(identity),
                              process_estimates=estimates, **evaluation))
    return dict(contract=CONTRACT, manifest_sha256=digest(canonical(declaration).encode()),
                r3_acceptance=False, n_accepted=False,
                acceptance_reason='Offline metric validation only; campaign/process independence and parallel controls are not certified',
                invalid_inputs=invalid, processes=processes, bands=bands,
                summary=dict(inputs=len(records), valid_identity_records=len(processes),
                             invalid_iterations=sum(not it['valid'] for p in processes
                                                    for c in p['configurations'] for it in c['iterations']),
                             band_statuses=dict(collections.Counter(b['status'] for b in bands))))


def read_record(path, expected):
    data = path.read_bytes()
    receipt = dict(path=str(path.resolve()), sha256=digest(data))
    require(re.fullmatch('[0-9a-f]{64}', expected) and receipt['sha256'] == expected,
            f'Input digest mismatch: {path}')

    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f'Duplicate JSON key: {key}')
            result[key] = value
        return result

    collection = json.loads(data, object_pairs_hook=unique,
                            parse_constant=lambda value: require(False, f'Non-finite JSON: {value}'))
    require(len(collection['runs']) == 1, 'Supply one original process record per input')
    return receipt, collection['runs'][0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--record', nargs=2, action='append', required=True, metavar=('PATH', 'SHA256'))
    parser.add_argument('--output', type=Path, required=True, help='New output directory; never overwrite evidence')
    parser.add_argument('--producer-root', type=Path, default=ROOT,
                        help='Original-source directory whose producer file digests must match')
    args = parser.parse_args()
    declaration = manifest(args.producer_root)
    records = [read_record(Path(path), expected) for path, expected in args.record]
    report = analyze(records, declaration)
    args.output.mkdir(parents=True, exist_ok=False)
    for name, value in (('manifest.json', declaration), ('report.json', report)):
        with (args.output / name).open('x') as stream:
            json.dump(value, stream, indent=2, allow_nan=False)
            stream.write('\n')
    print(json.dumps(report['summary'], indent=2))
    return 2 if report['invalid_inputs'] or report['summary']['invalid_iterations'] else 0


if __name__ == '__main__':
    raise SystemExit(main())
