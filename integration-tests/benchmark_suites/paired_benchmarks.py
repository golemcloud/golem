#!/usr/bin/env python3
"""Compare interleaved schema-2 benchmark process pairs without pooling raw requests."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import random
import signal
import statistics
import subprocess
import time


def schedule(pairs):
    if not 3 <= pairs <= 10:
        raise ValueError('Choose between three and ten process pairs')
    return [(pair, side) for pair in range(pairs) for side in ('base', 'head')]


def measurements(run):
    if run['schema_version'] != 2:
        raise ValueError('Paired comparisons require schema 2')
    values = {}
    for entry in run['results']:
        if not isinstance(entry['series_key'], str) or not entry['series_key']:
            raise ValueError('A comparison requires a known measurement series')
        for result in entry['results']:
            config = json.dumps(result['run_config'], sort_keys=True, separators=(',', ':'))
            iterations = result['iteration_results']
            if ([item['index'] for item in iterations] != list(range(entry['iterations']))
                    or any(item.get('skipped') or item['failures'] for item in iterations)
                    or result.get('failures')):
                raise ValueError('Incomplete or failed iteration evidence')
            metrics = set().union(*(item['duration_samples'] for item in iterations))
            for metric in sorted(metrics):
                if metric.endswith('__secondary'):
                    continue
                samples = [item['duration_samples'].get(metric, []) for item in iterations]
                if any(not group or any(not math.isfinite(v) or v <= 0 for v in group)
                       for group in samples):
                    raise ValueError('Missing or invalid duration evidence')
                key = (entry['name'], config, metric, run['harness_version'],
                       entry['entry_version'], entry['series_key'])
                if key in values:
                    raise ValueError('Duplicate measurement identity')
                values[key] = statistics.median(statistics.median(group) for group in samples)
    if not values:
        raise ValueError('No complete primary duration measurements')
    return values


def compare(records, base_source, head_source):
    """One observation per fresh suite process; resample matched process pairs."""
    expected = schedule(len(records) // 2)
    if [(item['pair'], item['side']) for item in records] != expected:
        raise ValueError('Expected complete interleaved base/head process pairs')
    runs = [item['run'] for item in records]
    if len({run['run_id'] for run in runs}) != len(runs):
        raise ValueError('A process result cannot serve as two independent observations')
    if any(not isinstance(run['suite_fingerprint'], str) or not run['suite_fingerprint']
           for run in runs):
        raise ValueError('A comparison requires a known suite definition')
    if (len({run['runner']['id'] for run in runs}) != 1
            or len({run['suite_fingerprint'] for run in runs}) != 1
            or len({run['artifacts']['runner']['sha256'] for run in runs}) != 1):
        raise ValueError('The comparison requires one runner, suite definition and harness artifact')
    observed = []
    for item in records:
        run = item['run']
        source = base_source if item['side'] == 'base' else head_source
        if run['source']['repository'] != 'golemcloud/golem' or run['source']['commitSha'] != source:
            raise ValueError('Incorrect paired source identity')
        observed.append(measurements(run))
    results = []
    keys = sorted(set().union(*observed))
    for key in keys:
        base = [item.get(key) for item in observed[0::2]]
        head = [item.get(key) for item in observed[1::2]]
        result = dict(entry=key[0], run_config=json.loads(key[1]), metric=key[2],
                      harness_version=key[3], entry_version=key[4], series_key=key[5],
                      base_observations=base, head_observations=head)
        if any(value is None for value in base + head):
            result['status'] = ('missing-in-base' if all(value is None for value in base)
                                else 'missing-in-head' if all(value is None for value in head)
                                else 'incomplete-pairs')
        else:
            ratios = [h / b - 1 for b, h in zip(base, head)]
            rng = random.Random(0)
            bootstrap = sorted(statistics.median(rng.choices(ratios, k=len(ratios)))
                               for _ in range(10000))
            interval = [bootstrap[249], bootstrap[9749]]
            result.update(relative_change=statistics.median(ratios),
                          confidence_interval=interval, paired_relative_changes=ratios,
                          status='regression' if interval[0] > 0 else
                                 'improvement' if interval[1] < 0 else 'inconclusive')
        results.append(result)
    return dict(base_source=base_source, head_source=head_source, process_pairs=len(expected) // 2,
                replication_unit='fresh suite process pair; median of iteration medians',
                interval_method='paired percentile bootstrap of median relative change',
                confidence_level=0.95, bootstrap_samples=10000, random_seed=0,
                results=results)


def save(path, value):
    with path.open('x') as stream:
        json.dump(value, stream, indent=2, allow_nan=False)


def artifact_manifest(root):
    files = [root / 'target/release' / name for name in (
        'worker-executor', 'golem-worker-service', 'golem-registry-service',
        'golem-shard-manager', 'golem-component-compilation-service')]
    files += sorted((root / 'test-components').glob('*.wasm'))
    for item in json.loads((root / 'builtin-artifacts.json').read_text()).values():
        path = (root / item['path']).resolve()
        if not path.is_relative_to(root):
            raise ValueError('Built-in artifact escapes its workspace')
        files.append(path)
    return {str(path.relative_to(root)): hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()
            for path in files}


def execute(args):
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise ValueError('Performance execution requires a GitHub Actions measurement runner')
    if not time.time() + 120 < args.deadline <= time.time() + 3600:
        raise ValueError('Supply an immutable future deadline within one hour')
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    roots = dict(base=args.base_root.resolve(), head=args.head_root.resolve())
    sources = dict(base=args.base_source, head=args.head_source)
    manifests = {}
    for side, root in roots.items():
        origin = json.loads(getattr(args, f'{side}_origin').read_text())
        if (origin['head_sha'] != sources[side] or origin['path'] != '.github/workflows/benchmark.yaml'
                or origin['event'] != 'workflow_dispatch'
                or origin['run_attempt'] != 1 or origin['conclusion'] != 'success'):
            raise ValueError('Bundle origin is not the successful exact-source CI attempt')
        actual = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
        if actual != sources[side]:
            raise ValueError('Service configuration checkout does not match the bundle source')
        manifests[side] = artifact_manifest(root)
        save(args.output / f'{side}-origin.json', origin)
    harness = (roots['head'] / 'target/benchmarks/benchmarks').resolve()
    harness_sha = hashlib.file_digest(harness.open('rb'), 'sha256').hexdigest()
    suite = args.suite.resolve()
    suite_sha = hashlib.file_digest(suite.open('rb'), 'sha256').hexdigest()
    planned = schedule(args.pairs)
    save(args.output / 'plan.json', dict(base_source=args.base_source, head_source=args.head_source,
         harness_source=args.head_source, harness_sha256=harness_sha, suite=str(suite),
         suite_sha256=suite_sha, deadline=args.deadline, runner=args.runner, schedule=planned,
         artifacts=manifests, publication=False))
    receipts = []
    records = []
    for ordinal, (pair, side) in enumerate(planned):
        if time.time() >= args.deadline - 120:
            raise TimeoutError('Paired measurement reached its cleanup reserve')
        root = roots[side]
        if (artifact_manifest(root) != manifests[side]
                or hashlib.file_digest(harness.open('rb'), 'sha256').hexdigest() != harness_sha
                or hashlib.file_digest(suite.open('rb'), 'sha256').hexdigest() != suite_sha):
            raise ValueError('Prepared artifacts or workload changed during the comparison')
        prefix = args.output / f'process-{ordinal}-{side}'
        result_path = prefix.with_suffix('.json')
        command = [str(harness), '--primary-only', '--retain-details', '--retain-selected-zero-counts',
                   'suite', '--run-id', f'paired-{os.environ["GITHUB_RUN_ID"]}-{ordinal}',
                   '--save-to-json', str(result_path), '--runner-id', args.runner,
                   '--source-repository', 'golemcloud/golem', '--source-commit-sha', sources[side],
                   '--source-ref', sources[side], str(suite), 'spawned',
                   '--workspace-root', str(root), '--component-directory', str(root / 'test-components'),
                   '--registry-service-http-port', '18081']
        environment = os.environ.copy()
        environment['GOLEM_BENCH_SOCKETS_ENABLED'] = 'false'
        environment['GOLEM__TRACING__OTLP__ENABLED'] = 'false'
        inventory = json.loads((root / 'builtin-artifacts.json').read_text())
        for name, item in inventory.items():
            environment[f'GOLEM__BUILTIN_ARTIFACTS__SOURCE_OVERRIDES__{name.upper()}__URL'] = (
                root / item['path']).as_uri()
        before = set(subprocess.check_output(['docker', 'ps', '-aq'], text=True, timeout=10).split())
        status = dict(pair=pair, side=side, command=command, started=time.time(), valid=False)
        process = None
        try:
            with prefix.with_suffix('.log').open('x') as log:
                process = subprocess.Popen(command, cwd=root, env=environment, stdout=log,
                                           stderr=subprocess.STDOUT, start_new_session=True)
                code = process.wait(timeout=min(480, args.deadline - time.time() - 120))
            status['exit_code'] = code
            if code:
                raise RuntimeError('Paired benchmark failed; no replacement is started')
            collection = json.loads(result_path.read_text())
            if len(collection['runs']) != 1:
                raise ValueError('Expected exactly one result per process')
            run = collection['runs'][0]
            measurements(run)
            if (run['source']['repository'] != 'golemcloud/golem'
                    or run['source']['commitSha'] != sources[side] or run['runner']['id'] != args.runner
                    or run['run_id'] != f'paired-{os.environ["GITHUB_RUN_ID"]}-{ordinal}'):
                raise ValueError('Measured result identity differs from the execution plan')
            artifacts = run['artifacts']
            if artifacts['runner']['sha256'] != harness_sha:
                raise ValueError('Measured harness differs from the fixed harness')
            if set(artifacts['services']) != {'golem-worker-executor', 'golem-worker-service',
                    'golem-registry-service', 'golem-shard-manager', 'golem-component-compilation-service'}:
                raise ValueError('Missing measured service provenance')
            for artifact in list(artifacts['services'].values()) + list(artifacts['fixtures'].values()):
                member = str(Path(artifact['path']).resolve().relative_to(root))
                if artifact['sha256'] != manifests[side][member]:
                    raise ValueError('Measured runtime differs from the prepared artifact')
            if time.time() >= args.deadline - 120 or artifact_manifest(root) != manifests[side]:
                raise ValueError('Expired observation or changed runtime artifacts')
            status['valid'] = True
        except BaseException as error:
            status.update(valid=False, error=repr(error))
            raise
        finally:
            handlers = {sig: signal.signal(sig, signal.SIG_IGN) for sig in (signal.SIGTERM, signal.SIGINT)}
            try:
                if process is not None:
                    for sig in (signal.SIGTERM, signal.SIGKILL):
                        try:
                            os.killpg(process.pid, sig)
                        except ProcessLookupError:
                            break
                        try:
                            process.wait(timeout=3)
                        except subprocess.TimeoutExpired:
                            continue
                containers = subprocess.check_output(
                    ['docker', 'ps', '-q'], text=True, timeout=10).split()
                status['remaining_containers'] = sorted(set(containers) - before)
                if status['valid'] and status['remaining_containers']:
                    raise RuntimeError('Containers remain after context cleanup; stop the comparison')
            except BaseException as error:
                status.update(valid=False, cleanup_error=repr(error))
                raise
            finally:
                status['ended'] = time.time()
                save(prefix.with_suffix('.status.json'), status)
                if status['valid']:
                    records.append(dict(pair=pair, side=side, run=run))
                    receipts.append(dict(pair=pair, side=side, file=result_path.name,
                        status_file=prefix.with_suffix('.status.json').name,
                        sha256=hashlib.file_digest(result_path.open('rb'), 'sha256').hexdigest()))
                save(args.output / f'ledger-{ordinal}.json', dict(
                    base_source=args.base_source, head_source=args.head_source, records=receipts,
                    complete=status['valid'] and ordinal == len(planned) - 1))
                for sig, handler in handlers.items():
                    signal.signal(sig, handler)
    save(args.output / 'comparison.json', compare(records, args.base_source, args.head_source))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    analysis = commands.add_parser('compare', help='Analyze saved results without measuring')
    analysis.add_argument('ledger', type=Path)
    analysis.add_argument('--output', type=Path, required=True)
    execution = commands.add_parser('run', help='Run prepared base/head artifacts on one CI runner')
    for side in ('base', 'head'):
        execution.add_argument(f'--{side}-root', type=Path, required=True)
        execution.add_argument(f'--{side}-source', required=True)
        execution.add_argument(f'--{side}-origin', type=Path, required=True)
    execution.add_argument('--suite', type=Path, required=True)
    execution.add_argument('--pairs', type=int, default=3)
    execution.add_argument('--deadline', type=int, required=True)
    execution.add_argument('--runner', default='github-actions-blacksmith-32vcpu',
                           choices=['github-actions-blacksmith-32vcpu'])
    execution.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'run':
        def interrupted(signum, frame):
            raise InterruptedError(f'Received signal {signum}')
        signal.signal(signal.SIGTERM, interrupted)
        signal.signal(signal.SIGINT, interrupted)
        execute(args)
        return
    ledger = json.loads(args.ledger.read_text())
    if not ledger['complete']:
        raise ValueError('Execution did not complete the planned process pairs')
    records = []
    for item in ledger['records']:
        result = (args.ledger.parent / item['file']).resolve()
        if not result.is_relative_to(args.ledger.parent.resolve()):
            raise ValueError('Result path escapes the evidence directory')
        digest = hashlib.file_digest(result.open('rb'), 'sha256').hexdigest()
        if digest != item['sha256']:
            raise ValueError('Saved paired result hash differs from the execution receipt')
        status_path = (args.ledger.parent / item['status_file']).resolve()
        if not status_path.is_relative_to(args.ledger.parent.resolve()):
            raise ValueError('Status path escapes the evidence directory')
        status = json.loads(status_path.read_text())
        if not status['valid'] or status['pair'] != item['pair'] or status['side'] != item['side']:
            raise ValueError('The process did not complete successfully with cleanup')
        runs = json.loads(result.read_text())['runs']
        if len(runs) != 1:
            raise ValueError('Expected exactly one run per process')
        records.append(dict(pair=item['pair'], side=item['side'], run=runs[0]))
    report = compare(records, ledger['base_source'], ledger['head_source'])
    with args.output.open('x') as stream:
        json.dump(report, stream, indent=2, allow_nan=False)


if __name__ == '__main__':
    main()
