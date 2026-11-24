#!/usr/bin/env python3
"""One fixed-budget CI campaign; saved evidence can be reconciled offline."""
import argparse
import collections
from datetime import datetime, timezone
import hashlib
import itertools
import json
import math
import os
from pathlib import Path
import re
import shutil
import signal
import statistics
import subprocess
import tarfile
import time

import r3_selector as selector
import validate_r3_metrics as metrics

original = selector.original
require, save, sha = metrics.require, original.save, original.sha
HERE = Path(__file__).resolve().parent
HARNESS = '4db6100d250b7cbfd2993d630d4454e09167cdc7'
HARNESS_SHA = 'f7773b69f648a7ae892cc1558826531b5ceda4a954fdfbf9416757c99184d8e0'
PRODUCER = 45359520345
ARTIFACT = 14903778434
PARTITIONS = ('startup', 'durability-echo', 'bulk', 'streaming')
INPUTS = ('benchmark-bundle.tar.gz', 'harness-overlay.tar.gz', 'harness-provenance.json',
          'original-origin.json', 'preparation-receipt.json', 'toolchain.txt')
WORKER_BYTES = 500 * 1024 ** 2
OTHER_BYTES = 1024 ** 3


def load(path):
    return json.loads(path.read_text())


def epoch(value):
    return datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp()


def suite(controls=False):
    return load(HERE / ('r3-controls.yaml' if controls else 'r3-full.yaml'))


def declaration(partition=None, controls=False):
    full = suite(controls)
    items = [b for b in full['benchmarks'] if partition is None or b['partition'] == partition]
    configurations, fixtures = [], set()
    for item in items:
        name = item['name']
        if name in ('cold-start-unknown-medium', 'latency-medium'):
            fixtures.add('benchmark_agent_ts')
        elif name == 'throughput-large-input-effect':
            fixtures.add('benchmark_agent_effect')
        elif name.removesuffix('-aggregate') in metrics.CALLS:
            fixtures.update(('benchmark_agent_rust_release', 'benchmark_agent_ts'))
        elif name == 'streaming-tool':
            fixtures.update(('golem_it_tool_streaming_rust_caller_release',
                             'golem_it_tool_streaming_rust_provider_release'))
        elif name.startswith('streaming-rpc'):
            fixtures.add('golem_it_agent_rpc_rust_release')
        else:
            fixtures.add('benchmark_agent_rust_release')
        cache = item.get('disableCompilationCache', False)
        entry = name + ('-no-cache' if cache else '')
        for cluster, size, length in itertools.product(item['clusterSize'], item['size'], item['length']):
            config = dict(clusterSize=cluster, size=size, length=length, disableCompilationCache=cache)
            configurations.append(dict(entry=entry, config=config, iterations=5, version=item['version'],
                                       **metrics.metric_rules(entry, config)))
    return dict(source=HARNESS, suite_name=full['name'],
                suite_fingerprint=metrics.digest(json.dumps(full, separators=(',', ':')).encode()),
                fixtures=sorted(fixtures), configurations=configurations)


def prepared_identity(prepared):
    selector.validate_origin(load(prepared / 'original-origin.json'))
    require(sha(prepared / 'benchmark-bundle.tar.gz') == original.BUNDLE_SHA, 'Original archive differs')
    receipt = load(prepared / 'preparation-receipt.json')
    require(receipt['archive_sha256'] == original.BUNDLE_SHA
            and receipt['overlay_sha256'] == sha(prepared / 'harness-overlay.tar.gz'), 'Overlay archive differs')
    with tarfile.open(prepared / 'harness-overlay.tar.gz') as archive:
        members = archive.getmembers()
        require(len(members) == 2 and {m.name for m in members} == {'benchmarks', 'harness-provenance.json'}
                and all(m.isfile() for m in members), 'Unsafe overlay')
        provenance = json.load(archive.extractfile('harness-provenance.json'))
        require(metrics.digest(archive.extractfile('benchmarks').read()) == HARNESS_SHA
                and archive.getmember('benchmarks').mode & 0o100, 'Wrong harness bytes')
    require(provenance == receipt['provenance'] == load(prepared / 'harness-provenance.json')
            and provenance['source'] == HARNESS and provenance['sha256'] == HARNESS_SHA
            and str(provenance['run_id']) == str(PRODUCER) and provenance['attempt'] == '1'
            and provenance['base_source'] == metrics.SOURCE
            and provenance['base_bundle_sha256'] == original.BUNDLE_SHA, 'Wrong harness producer')
    return {name: sha(prepared / name) for name in INPUTS}


def preflight(args):
    args.output.mkdir(parents=True, exist_ok=False)
    origin, campaign, artifact = load(args.origin), load(args.campaign_origin), load(args.artifact)
    require(origin['id'] == PRODUCER and origin['head_sha'] == HARNESS
            and origin['path'] == '.github/workflows/benchmark.yaml' and origin['run_attempt'] == 1
            and origin['conclusion'] == 'success' and origin['event'] == 'workflow_dispatch'
            and origin['repository']['full_name'] == 'golemcloud/golem', 'Wrong prepared origin')
    require(artifact['id'] == ARTIFACT and artifact['name'] == 'selector-preparation-attempt1'
            and artifact['size_in_bytes'] == 357345303 and artifact['expired'] is False
            and artifact['workflow_run']['id'] == PRODUCER
            and artifact['workflow_run']['head_sha'] == HARNESS, 'Wrong prepared artifact')
    require(campaign['path'] == '.github/workflows/benchmark.yaml' and campaign['run_attempt'] == 1
            and campaign['event'] == 'workflow_dispatch'
            and campaign['repository']['full_name'] == 'golemcloud/golem'
            and re.fullmatch('[0-9a-f]{40}', campaign['head_sha']), 'Wrong campaign origin')
    if os.environ.get('GITHUB_ACTIONS') == 'true':
        require(str(campaign['id']) == os.environ['GITHUB_RUN_ID']
                and campaign['head_sha'] == os.environ['GITHUB_SHA'], 'Campaign substituted')
    created = epoch(campaign['created_at'])
    require(time.time() < created + 18 * 60 and created + 570 * 60 < args.deadline <= created + 36000,
            'Insufficient original campaign budget')
    hashes = prepared_identity(args.prepared)
    for name in INPUTS:
        shutil.copyfile(args.prepared / name, args.output / name)
    for name, value in (('producer-origin.json', origin), ('campaign-origin.json', campaign),
                        ('prepared-artifact.json', artifact)):
        save(args.output / name, value)
    sources = {p.name: sha(p) for p in (Path(__file__), Path(metrics.__file__), Path(selector.__file__),
                                      Path(original.__file__), HERE / 'r3-full.yaml', HERE / 'r3-controls.yaml')}
    original_members = {}
    with tarfile.open(args.prepared / 'benchmark-bundle.tar.gz') as archive:
        for member in archive:
            path = Path(member.name)
            require(not path.is_absolute() and '..' not in path.parts
                    and (member.isfile() or member.isdir()), 'Unsafe original archive')
            if member.isfile():
                name = path.as_posix()
                require(name not in original_members, 'Duplicate original member')
                original_members[name] = hashlib.file_digest(archive.extractfile(member), 'sha256').hexdigest()
    save(args.output / 'campaign.json', dict(campaign_run_id=str(campaign['id']), created_epoch=created,
         deadline=args.deadline, orchestration_source=campaign['head_sha'], inputs=hashes, code=sources,
         original_members=original_members,
         harness_source=HARNESS, runtime_source=metrics.SOURCE, publication=False,
         allocation=dict(dollars=150, hours=10, waves=5, measurement_jobs=20,
                         measurement_billed_minutes=2160, small_billed_minutes=80,
                         storage_reserve_dollars=5, modeled_compute_dollars=138.56)))
    save(args.output / 'full-manifest.json', declaration())
    save(args.output / 'control-manifest.json', declaration(controls=True))
    require(directory_bytes(args.output) <= OTHER_BYTES, 'Input artifact budget exceeded')


def bound_campaign(prepared, live=False):
    campaign = load(prepared / 'campaign.json')
    for name, digest in campaign['inputs'].items():
        require(sha(prepared / name) == digest, f'Frozen input changed: {name}')
    for name, digest in campaign['code'].items():
        require(sha(HERE / name) == digest, f'Frozen interpretation changed: {name}')
    if live:
        require(os.environ.get('GITHUB_ACTIONS') == 'true' and os.environ['GITHUB_RUN_ATTEMPT'] == '1'
                and os.environ['GITHUB_SHA'] == campaign['orchestration_source']
                and os.environ['GITHUB_RUN_ID'] == campaign['campaign_run_id'], 'Wrong live campaign')
    return campaign


def wave_start(campaign, wave):
    require(type(wave) is int and 1 <= wave <= 5, 'Invalid fixed wave')
    return campaign['created_epoch'] + 1200 + (wave - 1) * 6600


def wait_barrier(target):
    require(time.time() <= target + 5, 'Missed immutable barrier')
    while time.time() < target:
        time.sleep(min(1, target - time.time()))
    require(time.time() <= target + 5, 'Late process start')


def directory_bytes(path):
    return sum(p.stat().st_size for p in path.rglob('*') if p.is_file())


def terminate(process):
    if process is not None:
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.killpg(process.pid, sig)
            except ProcessLookupError:
                break
            if sig == signal.SIGTERM:
                time.sleep(3)
        process.wait(timeout=3)


def process_run(command, root, output, start, cutoff, environment):
    """Only the owned process group is signalled, including after leader exit."""
    output.mkdir()
    status = dict(valid=False, barrier=start, cutoff=cutoff)
    process = None
    try:
        wait_barrier(start)
        with (output / 'process.log').open('x') as log:
            status.update(started=time.time(), monotonic_started=time.monotonic())
            require(status['started'] <= start + 5, 'Late dispatch')
            process = subprocess.Popen(command, cwd=root, env=environment, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            status['pid'] = process.pid
            while process.poll() is None:
                require(time.time() < cutoff - 20, 'Phase cleanup reserve reached')
                require(directory_bytes(output.parent) <= WORKER_BYTES, 'Worker evidence budget exceeded')
                try:
                    process.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    pass
        status.update(ended=time.time(), monotonic_ended=time.monotonic(), exit_code=process.returncode)
        require(process.returncode == 0, 'Measurement failed; no replacement')
        require(abs((status['ended'] - status['started'])
                    - (status['monotonic_ended'] - status['monotonic_started'])) <= 5, 'Clock discontinuity')
        status['valid'] = True
    except BaseException as error:
        status['error'] = repr(error)
        raise
    finally:
        handlers = {sig: signal.signal(sig, signal.SIG_IGN) for sig in (signal.SIGTERM, signal.SIGINT)}
        try:
            terminate(process)
            remaining = subprocess.check_output(['docker', 'ps', '-q'], text=True, timeout=10).split()
            status['remaining_containers'] = remaining
            require(not remaining, 'Container remains; stop, runner teardown owns cleanup')
            require(time.time() < cutoff, 'Phase exceeded immutable cleanup cutoff')
            require(directory_bytes(output.parent) <= WORKER_BYTES, 'Worker evidence budget exceeded')
        except BaseException as error:
            status.update(valid=False, cleanup_error=repr(error))
            raise
        finally:
            status['cleanup_ended'] = time.time()
            save(output / 'status.json', status)
            for sig, handler in handlers.items():
                signal.signal(sig, handler)


def selection_rows(log):
    """Pretty INFO events carry the configuration in their enclosing span stack."""
    text = re.sub(r'(?:\x1b|\\u001b)\[[0-9;]*m', '', log)
    rows = []
    for block in re.split(r'\n\s*\n', text):
        if not re.search(r'rpc_selection(?:=|:\s*)', block):
            continue
        receipts = selector.selections(block)
        require(len(receipts) == 1, 'Ambiguous selection event')
        context = re.search(r'name: "([^"]+)", cluster_size: (\d+), size: (\d+), length: (\d+)', block)
        require(context is not None, 'Missing selection configuration span')
        row = receipts[0]
        row['context'] = [context[1], *map(int, context.groups()[1:])]
        rows.append(row)
    return rows


def validate_selections(rows, declared):
    expected = {}
    for spec in declared['configurations']:
        if spec['entry'].removesuffix('-aggregate') not in metrics.CALLS:
            continue
        c = spec['config']
        for index in range(5):
            expected[(spec['entry'], c['clusterSize'], c['size'], c['length'], index)] = 2
    groups = collections.defaultdict(list)
    for row in rows:
        key = (*row['context'], row['iteration'])
        require(key in expected and type(row['iteration']) is int, 'Unexpected selection configuration')
        require(type(row['target']) is int and 0 <= row['target'] < key[2]
                and type(row['candidate']) is int and 0 <= row['candidate'] < 10000, 'Invalid selection target')
        local = key[1] == 1 or row['iteration'] % 2 == row['target'] % 2
        identity = f'("rpc-test-{row["target"]}-{row["candidate"]}")'
        language = 'rust' if row['parent'].startswith('RustRpcBenchmarkAgent(') else 'ts'
        parent, child = ('RustRpcBenchmarkAgent', 'RustBenchmarkAgent') if language == 'rust' else ('RpcBenchmarkAgent', 'BenchmarkAgent')
        require(row['local'] is local and (row['parent_pod'] == row['child_pod']) == local
                and row['parent'] == parent + identity and row['child'] == child + identity,
                'Wrong selected route or identity')
        groups[row['component_id']].append(row)
    inventory = collections.Counter()
    for group in groups.values():
        key = (*group[0]['context'], group[0]['iteration'])
        require(all((*row['context'], row['iteration']) == key for row in group)
                and len(group) == key[2] and {r['target'] for r in group} == set(range(key[2])),
                'Missing/duplicate selection target or reused component')
        inventory[(key, group[0]['parent'].startswith('RustRpcBenchmarkAgent('))] += 1
    require(inventory == collections.Counter({(key, rust): 1 for key in expected for rust in (False, True)}),
            'Incomplete selection inventory')
    return dict(groups=len(groups), pairs=len(rows))


def artifacts_match(run, members):
    require(run['artifacts']['runner']['sha256'] == HARNESS_SHA, 'Wrong measured harness')
    pairs = [(run['artifacts']['runner'], 'target/benchmarks/benchmarks')]
    pairs += [(item, 'target/release/' + ('worker-executor' if name == 'golem-worker-executor' else name))
              for name, item in run['artifacts']['services'].items()]
    pairs += [(item, f'test-components/{name}.wasm') for name, item in run['artifacts']['fixtures'].items()]
    for item, member in pairs:
        require(item['sha256'] == members[member], f'Measured artifact differs: {member}')
    require(all(e['measurement_environment']['hardware_fingerprint'] == original.HARDWARE
                for e in run['results']), 'Wrong hardware class')


def evidence_record(path, declared, members, run_id):
    record = metrics.read_record(path, sha(path))
    require(record[1]['run_id'] == run_id, 'Wrong process identity')
    artifacts_match(record[1], members)
    analysis = metrics.analyze([record], declared)
    require(not analysis['invalid_inputs'] and analysis['summary']['invalid_iterations'] == 0,
            f'Invalid process metrics: {analysis["invalid_inputs"]}')
    require(analysis['summary']['valid_identity_records'] == 1, 'Missing process')
    configurations, _ = metrics.validate_identity(record[1], declared)
    for spec in declared['configurations']:
        if spec['entry'] not in metrics.CALLS:
            continue
        raw = configurations[metrics.configuration_key(spec['entry'], spec['config'])]
        for iteration in raw['iteration_results']:
            for language in ('rust', 'ts'):
                for target in range(spec['config']['size']):
                    local = spec['config']['clusterSize'] == 1 or iteration['index'] % 2 == target % 2
                    route = 'local' if local else 'remote'
                    key = f'{language}-agent-rpc-{route}-worker-{target}__secondary'
                    require(len(iteration['duration_samples'].get(key, [])) == metrics.CALLS[spec['entry']],
                            'Wrong per-target selected route despite matching primary totals')
    for config in analysis['processes'][0]['configurations']:
        for metric in config['metrics'].values():
            require(metric['state'] != 'not-applicable' or metric['structural_na'], 'Missing possible exposure')
            if metric['estimate'] is not None and metric['quantity'] != 'event-count':
                require(metric['estimate'] > 0, 'Under-resolution metric')
    return record, analysis


def run(args):
    args.output.mkdir(parents=True, exist_ok=False)
    status = dict(valid=False, wave=args.wave, partition=args.partition, started=time.time(), publication=False)
    def interrupted(signum, frame):
        raise InterruptedError(f'Interrupted by signal {signum}')
    handlers = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGTERM, signal.SIGINT)}
    try:
        campaign = bound_campaign(args.prepared, live=True)
        start = wave_start(campaign, args.wave)
        def checked(command, limit=120):
            remaining = start + 600 - time.time() - 15
            require(remaining > 0, 'Setup budget exhausted')
            return subprocess.check_output(command, cwd=args.root, text=True, timeout=min(limit, remaining))
        require(checked(['git', 'rev-parse', 'HEAD']).strip() == metrics.SOURCE, 'Wrong runtime checkout')
        prepared_identity(args.prepared)
        members = original.install(args.prepared / 'benchmark-bundle.tar.gz', args.root)
        require(members == campaign['original_members'], 'Original member inventory differs')
        save(args.output / 'original-members.json', members)
        selector.install_overlay(args.prepared / 'harness-overlay.tar.gz', args.root, members, HARNESS, str(PRODUCER))
        save(args.output / 'installed-members.json', members)
        save(args.output / 'artifact-sources.json', {
            name: HARNESS if name == 'target/benchmarks/benchmarks' else metrics.SOURCE for name in members})
        require(not checked(['docker', 'ps', '-aq']).split(), 'Runner is not fresh')
        require('v=8.0.2 ' in checked(['redis-server', '--version']), 'Wrong Redis')
        checked(['docker', 'pull', original.POSTGRES], 300)
        checked(['docker', 'tag', original.POSTGRES, 'postgres:17.7'])
        save(args.output / 'environment.json', dict(redis=checked(['redis-server', '--version']),
             kernel=checked(['uname', '-a']), docker=checked(['docker', '--version']),
             postgres=checked(['docker', 'image', 'inspect', 'postgres:17.7', '--format', '{{json .RepoDigests}}'])))
        harness = args.root / 'target/benchmarks/benchmarks'
        for control in (False, True):
            checked([str(harness), 'suite', '--check-artifacts', str(HERE / ('r3-controls.yaml' if control else 'r3-full.yaml')), 'spawned'])
        environment = dict(os.environ, GOLEM_BENCH_SOCKETS_ENABLED='false', GOLEM__TRACING__OTLP__ENABLED='false')
        for name, item in load(args.root / 'builtin-artifacts.json').items():
            environment[f'GOLEM__BUILTIN_ARTIFACTS__SOURCE_OVERRIDES__{name.upper()}__URL'] = (args.root / item['path']).as_uri()
        for phase, begin, end in (('pre', 600, 840), ('main', 840, 5940), ('post', 5940, 6180)):
            controls = phase != 'main'
            output = args.output / phase
            declared = declaration(controls=controls, partition=None if controls else args.partition)
            run_id = f'r3-{campaign["campaign_run_id"]}-w{args.wave}-{args.partition}-{phase}'
            timestamp = datetime.fromtimestamp(start + begin, timezone.utc).isoformat().replace('+00:00', 'Z')
            command = [str(harness), '--primary-only', '--retain-details', '--retain-selected-zero-counts',
                       'suite', '--run-id', run_id, '--timestamp', timestamp, '--save-to-json', str(output / 'raw.json'),
                       '--runner-id', metrics.RUNNER, '--source-repository', 'golemcloud/golem',
                       '--source-commit-sha', HARNESS, '--source-ref', HARNESS]
            if not controls:
                command += ['--partition', args.partition]
            command += [str(HERE / ('r3-controls.yaml' if controls else 'r3-full.yaml')), 'spawned',
                        '--workspace-root', str(args.root), '--component-directory', str(args.root / 'test-components'),
                        '--registry-service-http-port', '18081']
            save(args.output / f'{phase}-plan.json', dict(command=command, manifest=declared,
                 orchestration_source=campaign['orchestration_source'], runtime_source=metrics.SOURCE))
            process_run(command, args.root, output, start + begin, start + end, environment)
            record, analysis = evidence_record(output / 'raw.json', declared, members, run_id)
            for name, digest in members.items():
                require(sha(args.root / name) == digest, f'Installed member changed: {name}')
            artifact_paths = [record[1]['artifacts']['runner'], *record[1]['artifacts']['services'].values(),
                              *record[1]['artifacts']['fixtures'].values()]
            require(all(Path(item['path']).resolve().is_relative_to(args.root) for item in artifact_paths),
                    'Measured artifact outside runtime root')
            save(output / 'analysis.json', analysis)
            save(output / 'selection-coverage.json', validate_selections(selection_rows((output / 'process.log').read_text()), declared))
            require(directory_bytes(args.output) <= WORKER_BYTES, 'Worker evidence budget exceeded')
        require(time.time() < start + 6240, 'Evidence must be ready before upload reserve')
        status['valid'] = True
    except BaseException as error:
        status['error'] = repr(error)
        raise
    finally:
        status['ended'] = time.time()
        save(args.output / 'status.json', status)
        for sig, handler in handlers.items():
            signal.signal(sig, handler)


def control_agreement(analyses, intervals):
    require(len(analyses) == len(intervals) == 4, 'Four comparable control processes required')
    require(max(s['started'] for s in intervals) < min(s['ended'] for s in intervals),
            'No positive four-process control overlap')
    values = collections.defaultdict(list)
    for analysis in analyses:
        for config in analysis['processes'][0]['configurations']:
            for name, metric in config['metrics'].items():
                if metric['structural_na']:
                    require(metric['estimate'] is None, 'Structural N/A supplied an estimate')
                    continue
                require(metric['estimate'] is not None, 'Missing mandatory control metric')
                key = metrics.canonical([config['entry'], config['config'], name])
                values[key].append((metric['quantity'], metric['estimate']))
    comparisons = []
    for key, observations in sorted(values.items()):
        require(len(observations) == 4 and len({q for q, _ in observations}) == 1, 'Control metric disagreement')
        numbers = [value for _, value in observations]
        low, high = min(numbers), max(numbers)
        ratio = high / low if low else (1.0 if high == 0 and observations[0][0] == 'event-count' else None)
        require(ratio is not None and math.isfinite(ratio) and ratio <= 1.20,
                f'Parallel controls exceed max/min1.20: {key}: {numbers}')
        comparisons.append(dict(metric=json.loads(key), estimates=numbers, max_min=ratio))
    return comparisons


def budget(campaign, jobs, artifacts, wave):
    jobs = [j for page in jobs for j in page['jobs']] if isinstance(jobs, list) else jobs['jobs']
    artifacts = [a for page in artifacts for a in page['artifacts']] if isinstance(artifacts, list) else artifacts['artifacts']
    large, small = [], []
    for job in jobs:
        # The platform labels remain visible on completed jobs; never infer size from duration.
        if job.get('conclusion') == 'skipped':
            continue
        is_large = 'blacksmith-32vcpu-ubuntu-2204' in job['labels']
        is_small = 'blacksmith-2vcpu-ubuntu-2204' in job['labels']
        if job['status'] == 'queued' and not job.get('started_at'):
            continue
        require(is_large != is_small, 'Unknown billed runner class')
        require(job['run_attempt'] == 1 and str(job['run_id']) == campaign['campaign_run_id'], 'Wrong job attempt')
        if is_large:
            require(job['status'] == 'completed' and job['conclusion'] == 'success', 'Incomplete measurement job')
        seconds = max(0, (epoch(job['completed_at']) if job.get('completed_at') else time.time()) - epoch(job['started_at']))
        (large if is_large else small).append(seconds)
    require(len(large) == wave * 4 and len(small) <= 10 and sum(small) <= 70 * 60, 'Job-count or small-runner budget exceeded')
    require(all(v <= 107 * 60 for v in large), 'Measurement job cap exceeded')
    measured_minutes = sum(math.ceil(v / 60) + 1 for v in large)
    small_minutes = sum(math.ceil(v / 60) + 1 for v in small)
    projected = (measured_minutes + (20 - len(large)) * 108) * .064 + max(80, small_minutes) * .004 + 5
    require(projected <= 150, 'Observed plus remaining allocation exceeds$150')
    workers = [a for a in artifacts if re.fullmatch(r'r3-wave[1-5]-(startup|durability-echo|bulk|streaming)-attempt1', a['name'])]
    require(all(a['size_in_bytes'] <= WORKER_BYTES for a in workers)
            and sum(a['size_in_bytes'] for a in artifacts if a not in workers) <= OTHER_BYTES, 'Artifact budget exceeded')
    return dict(large_jobs=len(large), small_jobs=len(small), measured_billed_minutes=measured_minutes,
                observed_compute_dollars=measured_minutes * .064 + small_minutes * .004,
                projected_with_remaining_and_storage_dollars=projected,
                provider_invoice_verified=False)


def wave_evidence(prepared, evidence, wave):
    campaign = bound_campaign(prepared)
    start = wave_start(campaign, wave)
    records, control_reports, intervals = {}, {'pre': [], 'post': []}, {'pre': [], 'post': []}
    for partition in PARTITIONS:
        root = evidence / f'r3-wave{wave}-{partition}-attempt1'
        require(directory_bytes(root) <= WORKER_BYTES, 'Worker evidence cap exceeded')
        status = load(root / 'status.json')
        require(status['valid'] and status['wave'] == wave and status['partition'] == partition
                and start <= status['started'] < start + 600 and status['ended'] < start + 6240,
                'Incomplete or late worker')
        original_members, members = load(root / 'original-members.json'), load(root / 'installed-members.json')
        require(original_members == campaign['original_members']
                and members == dict(original_members, **{'target/benchmarks/benchmarks': HARNESS_SHA}),
                'Unexpected original or overlay members')
        for phase, begin, end in (('pre', 600, 840), ('main', 840, 5940), ('post', 5940, 6180)):
            path = root / phase
            timing = load(path / 'status.json')
            require(timing['valid'] and timing['exit_code'] == 0 and not timing['remaining_containers']
                    and start + begin <= timing['started'] <= start + begin + 5
                    and timing['started'] < timing['ended'] <= timing['cleanup_ended'] < start + end,
                    'Invalid phase timing/cleanup')
            declared = declaration(controls=phase != 'main', partition=partition if phase == 'main' else None)
            record, report = evidence_record(path / 'raw.json', declared, members,
                f'r3-{campaign["campaign_run_id"]}-w{wave}-{partition}-{phase}')
            validate_selections(selection_rows((path / 'process.log').read_text()), declared)
            if phase == 'main':
                records[partition] = record
            else:
                control_reports[phase].append(report)
                intervals[phase].append(timing)
    comparisons = {phase: control_agreement(control_reports[phase], intervals[phase]) for phase in ('pre', 'post')}
    changes = []
    for pre, post in zip(comparisons['pre'], comparisons['post']):
        require(pre['metric'] == post['metric'], 'Pre/post metric inventories differ')
        ratios = []
        for before, after in zip(pre['estimates'], post['estimates']):
            ratio = after / before if before else None
            ratios.append(ratio if ratio is not None and math.isfinite(ratio) else None)
        changes.append(dict(metric=pre['metric'], post_over_pre=ratios,
                            qualification='Null when baseline zero or ratio unrepresentable; no extra threshold'))
    return records, dict(controls=comparisons, intervals=intervals, host_pre_post_changes=changes)


def reconcile(args, final=False):
    args.output.mkdir(parents=True, exist_ok=False)
    report = dict(valid=False, r3_acceptance=False, n_accepted=False, publication=False)
    try:
        campaign = bound_campaign(args.prepared)
        waves = range(1, 6) if final else (args.wave,)
        all_records = collections.defaultdict(list)
        for wave in waves:
            records, comparisons = wave_evidence(args.prepared, args.evidence, wave)
            save(args.output / f'wave{wave}.json', comparisons)
            for partition, record in records.items():
                all_records[partition].append(record)
        report['budget'] = budget(campaign, load(args.jobs), load(args.artifacts), max(waves))
        if not final:
            require(time.time() < wave_start(campaign, args.wave) + 6480, 'Wave gate deadline exceeded')
        else:
            statuses = collections.Counter()
            for partition, records in all_records.items():
                result = metrics.analyze(records, declaration(partition))
                for band in result['bands']:
                    values = [v for v in band['process_estimates'].values() if v is not None]
                    if values:
                        scale = max(values)
                        band.update(minimum=min(values), median=metrics.median(values), maximum=max(values),
                                    sample_deviation=(statistics.stdev([v / scale for v in values]) * scale
                                                      if scale and len(values) > 1 else 0))
                save(args.output / f'{partition}-noise.json', result)
                statuses.update(result['summary']['band_statuses'])
            report.update(band_statuses=dict(statuses),
                          noise_criteria_met=set(statuses) <= {'within-noise-threshold', 'not-applicable'},
                          qualification='Review required; process overlap is not measured-request simultaneity')
        report['valid'] = True
        artifacts = load(args.artifacts)
        artifacts = [a for page in artifacts for a in page['artifacts']] if isinstance(artifacts, list) else artifacts['artifacts']
        other_bytes = sum(a['size_in_bytes'] for a in artifacts if not a['name'].startswith('r3-wave'))
        require(other_bytes + directory_bytes(args.output) + 1024 * 1024 <= OTHER_BYTES,
                'Inputs/reports/receipts aggregate budget exceeded')
    except BaseException as error:
        report['valid'] = False
        report['error'] = repr(error)
        raise
    finally:
        save(args.output / ('final.json' if final else 'gate.json'), report)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='mode', required=True)
    frozen = commands.add_parser('preflight')
    for name in ('origin', 'campaign-origin', 'artifact'):
        frozen.add_argument('--' + name, type=Path, required=True)
    frozen.add_argument('--deadline', type=int, required=True)
    worker = commands.add_parser('run')
    worker.add_argument('--root', type=lambda s: Path(s).resolve(), required=True)
    worker.add_argument('--partition', choices=PARTITIONS, required=True)
    gate = commands.add_parser('gate')
    final = commands.add_parser('final')
    for command in (worker, gate):
        command.add_argument('--wave', type=int, required=True)
    for command in (gate, final):
        for name in ('evidence', 'jobs', 'artifacts'):
            command.add_argument('--' + name, type=Path, required=True)
    for command in (frozen, worker, gate, final):
        command.add_argument('--prepared', type=Path, required=True)
        command.add_argument('--output', type=lambda s: Path(s).resolve(), required=True)
    args = parser.parse_args()
    if args.mode == 'preflight':
        preflight(args)
    elif args.mode == 'run':
        run(args)
    else:
        reconcile(args, final=args.mode == 'final')


if __name__ == '__main__':
    main()
