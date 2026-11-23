#!/usr/bin/env python3
"""Bounded CI routing-coverage pilot; no R3 acceptance, rebuild or publication path."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tarfile
import time

import validate_r3_metrics as metrics

ROOT = Path(__file__).resolve().parents[2]
BUNDLE_SHA = '2e36ecaedaa902af7ef29c2b799d45697acd68b4e625ec768cfc2e7fcaf3db92'
HARDWARE = 'b9972b8ef6f1f0bcc751e190380e0927a384d3855a71aba2790c23ceb23d1ff3'
POSTGRES = 'postgres@sha256:2006493727bd5277eece187319af1ef82b4cf82cf4fc1ed00da0775b646ac2a4'
CASES = (('throughput-cpu-intensive', 1, 100), ('throughput-cpu-intensive', 10, 100),
         ('throughput-large-input', 1, 100), ('throughput-large-input', 1, 10000))


def suite():
    return dict(name='R3 coverage pilot', benchmarks=[
        dict(name='throughput-cpu-intensive', version=1, iterations=5, clusterSize=[5], size=[1, 10], length=[100]),
        dict(name='throughput-large-input', version=1, iterations=5, clusterSize=[5], size=[1], length=[100, 10000])])


def declaration():
    result = metrics.manifest()
    result.update(suite_name=suite()['name'],
                  suite_fingerprint=metrics.digest(json.dumps(suite(), separators=(',', ':')).encode()),
                  fixtures=['benchmark_agent_rust_release', 'benchmark_agent_ts'])
    configurations = []
    for name, size, length in CASES:
        item = copy.deepcopy(next(s for s in result['configurations'] if s['entry'] == name
                                 and s['config']['clusterSize'] == 5 and s['config']['size'] == size
                                 and s['config']['length'] == length))
        item['iterations'] = 5
        configurations.append(item)
    result['configurations'] = configurations
    return result


def analyze(run):
    declared = declaration()
    configurations, _ = metrics.validate_identity(run, declared)
    rows = []
    for specification in declared['configurations']:
        key = metrics.configuration_key(specification['entry'], specification['config'])
        raw = configurations[key]
        views = [metrics.iteration_view(it, specification, set()) for it in raw['iteration_results']]
        metrics.require(all(view['valid'] for view in views), f'Invalid iteration: {key}')
        for language in ('rust', 'ts'):
            for route in ('local', 'remote'):
                mask = [view['index'] for view in views if view['exposures'][f'{language}/{route}'] > 0]
                rows.append(dict(entry=specification['entry'], config=specification['config'],
                                 language=language, route=route, applicable_indexes=mask,
                                 exposed=bool(mask)))
    return dict(run_id=run['run_id'], r3_acceptance=False, n_accepted=False, rows=rows,
                coverage_complete=all(row['exposed'] for row in rows),
                qualification='Coverage feasibility only; no full-suite noise or parallel-control claim')


def save(path, value):
    with path.open('x') as stream:
        json.dump(value, stream, indent=2, allow_nan=False)


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def install(archive, root):
    metrics.require(sha(archive) == BUNDLE_SHA, 'Original archive hash mismatch')
    members = {}
    with tarfile.open(archive) as tar:
        for member in tar:
            path = Path(member.name)
            metrics.require(not path.is_absolute() and '..' not in path.parts, 'Unsafe archive path')
            metrics.require(member.isfile() or member.isdir(), 'Unexpected archive member type')
            if member.isfile():
                metrics.require(path.as_posix() not in members, 'Duplicate archive member')
                members[path.as_posix()] = hashlib.file_digest(tar.extractfile(member), 'sha256').hexdigest()
        tar.extractall(root, filter='data')
    for name, expected in members.items():
        metrics.require(sha(root / name) == expected, 'Installed member hash mismatch')
    metrics.require(os.access(root / 'target/benchmarks/benchmarks', os.X_OK), 'Harness not executable')
    return members


def execute(args):
    metrics.require(os.environ.get('GITHUB_ACTIONS') == 'true', 'Measurement requires GitHub Actions')
    metrics.require(os.environ['GITHUB_RUN_ATTEMPT'] == '1', 'No reruns or replacements')
    metrics.require(time.time() + 120 < args.deadline <= time.time() + 1800, 'Invalid immutable deadline')
    args.output.mkdir(parents=True, exist_ok=False)
    status = dict(valid=False, started=time.time(), deadline=args.deadline, publication=False)
    process = None

    def interrupted(signum, frame):
        raise InterruptedError(f'Interrupted by signal {signum}')

    handlers = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGTERM, signal.SIGINT)}

    def checked(command, limit=120):
        remaining = args.deadline - time.time() - 120
        metrics.require(remaining > 0, 'Cleanup reserve reached')
        return subprocess.check_output(command, cwd=args.root, text=True, timeout=min(limit, remaining))

    try:
        metrics.require(checked(['git', 'rev-parse', 'HEAD']).strip() == metrics.SOURCE,
                        'Runtime configuration checkout differs')
        origin = json.loads(args.origin.read_text())
        metrics.require(origin['id'] == 44078542455 and origin['head_sha'] == metrics.SOURCE
                        and origin['path'] == '.github/workflows/benchmark.yaml'
                        and origin['event'] == 'workflow_dispatch' and origin['run_attempt'] == 1
                        and origin['conclusion'] == 'success', 'Wrong original producer workflow')
        save(args.output / 'original-origin.json', origin)
        members = install(args.archive, args.root)
        save(args.output / 'members.json', members)
        metrics.require(not checked(['docker', 'ps', '-aq']).split(), 'Runner is not fresh/exclusive')
        redis = checked(['redis-server', '--version'])
        metrics.require('v=8.0.2 ' in redis, 'Wrong Redis version')
        checked(['docker', 'pull', POSTGRES], 300)
        checked(['docker', 'tag', POSTGRES, 'postgres:17.7'])
        suite_path = args.output / 'suite.yaml'
        save(suite_path, suite())
        save(args.output / 'manifest.json', declaration())
        harness = args.root / 'target/benchmarks/benchmarks'
        checked([str(harness), 'suite', '--check-artifacts', str(suite_path), 'spawned'])
        run_id = f'r3-coverage-{os.environ["GITHUB_RUN_ID"]}-{args.host}'
        command = [str(harness), '--primary-only', '--retain-details', '--retain-selected-zero-counts',
                   'suite', '--run-id', run_id, '--save-to-json', str(args.output / 'raw.json'),
                   '--runner-id', metrics.RUNNER, '--source-repository', 'golemcloud/golem',
                   '--source-commit-sha', metrics.SOURCE, '--source-ref', metrics.SOURCE,
                   str(suite_path), 'spawned', '--workspace-root', str(args.root),
                   '--component-directory', str(args.root / 'test-components'),
                   '--registry-service-http-port', '18081']
        environment = dict(os.environ, GOLEM_BENCH_SOCKETS_ENABLED='false', GOLEM__TRACING__OTLP__ENABLED='false')
        for name, item in json.loads((args.root / 'builtin-artifacts.json').read_text()).items():
            environment[f'GOLEM__BUILTIN_ARTIFACTS__SOURCE_OVERRIDES__{name.upper()}__URL'] = (
                args.root / item['path']).as_uri()
        save(args.output / 'plan.json', dict(command=command, archive_sha256=BUNDLE_SHA,
             source=metrics.SOURCE, orchestration=os.environ['GITHUB_SHA'], host=args.host,
             suite_sha256=sha(suite_path), deadline=args.deadline, max_process_seconds=900,
             max_log_bytes=134217728, publication=False))
        log_path = args.output / 'process.log'
        with log_path.open('x') as log:
            status['measurement_started'] = time.time()
            process = subprocess.Popen(command, cwd=args.root, env=environment, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            cutoff = min(time.time() + 900, args.deadline - 120)
            while process.poll() is None:
                metrics.require(time.time() < cutoff, 'Measurement budget exhausted')
                metrics.require(log_path.stat().st_size <= 134217728, 'Log budget exhausted')
                try:
                    process.wait(timeout=min(1, max(0.01, cutoff - time.time())))
                except subprocess.TimeoutExpired:
                    pass
        status.update(measurement_ended=time.time(), exit_code=process.returncode)
        metrics.require(process.returncode == 0, 'Benchmark failed; no replacement')
        metrics.require(time.time() < cutoff, 'Measurement completed outside its budget')
        collection = json.loads((args.output / 'raw.json').read_text())
        metrics.require(len(collection['runs']) == 1, 'Expected one fresh process')
        run = collection['runs'][0]
        metrics.require(run['run_id'] == run_id, 'Wrong process identity')
        for entry in run['results']:
            metrics.require(entry['measurement_environment']['hardware_fingerprint'] == HARDWARE,
                            'Hardware class differs from the retained cohort')
        artifacts = run['artifacts']
        metrics.require(artifacts['runner']['sha256'] == members['target/benchmarks/benchmarks'], 'Wrong harness')
        for item in list(artifacts['services'].values()) + list(artifacts['fixtures'].values()):
            member = str(Path(item['path']).resolve().relative_to(args.root))
            metrics.require(item['sha256'] == members[member] == sha(args.root / member), 'Wrong measured artifact')
        report = analyze(run)
        save(args.output / 'coverage.json', report)
        status['valid'] = True
    except BaseException as error:
        status['error'] = repr(error)
        raise
    finally:
        for sig in handlers:
            signal.signal(sig, signal.SIG_IGN)
        try:
            if process is not None:
                for sig in (signal.SIGTERM, signal.SIGKILL):
                    try:
                        os.killpg(process.pid, sig)
                    except ProcessLookupError:
                        break
                    if sig == signal.SIGTERM:
                        try:
                            process.wait(timeout=3)
                        except subprocess.TimeoutExpired:
                            pass
                process.wait(timeout=3)
            remaining = subprocess.check_output(['docker', 'ps', '-q'], text=True, timeout=10).split()
            status['remaining_containers'] = remaining
            metrics.require(not remaining, 'Container remains; stop progression and leave teardown to the runner')
        except BaseException as error:
            status.update(valid=False, cleanup_error=repr(error))
            raise
        finally:
            status['ended'] = time.time()
            save(args.output / 'status.json', status)
            for sig, handler in handlers.items():
                signal.signal(sig, handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='mode', required=True)
    run = commands.add_parser('run')
    run.add_argument('--root', type=lambda s: Path(s).resolve(), required=True)
    run.add_argument('--archive', type=Path, required=True)
    run.add_argument('--origin', type=Path, required=True)
    run.add_argument('--output', type=lambda s: Path(s).resolve(), required=True)
    run.add_argument('--deadline', type=int, required=True)
    run.add_argument('--host', choices=('1', '2', '3', '4'), required=True)
    analysis = commands.add_parser('analyze')
    analysis.add_argument('record', type=Path)
    args = parser.parse_args()
    if args.mode == 'run':
        execute(args)
    else:
        collection = json.loads(args.record.read_text())
        metrics.require(len(collection['runs']) == 1, 'Expected one fresh process')
        print(json.dumps(analyze(collection['runs'][0]), indent=2, allow_nan=False))


if __name__ == '__main__':
    main()
