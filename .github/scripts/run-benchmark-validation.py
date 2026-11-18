#!/usr/bin/env python3
"""Run a finite diagnostic with an immutable bundle and a shared UTC deadline."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import tarfile
import time
import urllib.parse
import urllib.request

import yaml

SOURCE = 'eb46c797c329e1787fb9260cc57b132e0830489d'
BUNDLE_SHA = '2e36ecaedaa902af7ef29c2b799d45697acd68b4e625ec768cfc2e7fcaf3db92'
POSTGRES = 'postgres@sha256:2006493727bd5277eece187319af1ef82b4cf82cf4fc1ed00da0775b646ac2a4'
ROOT = Path(__file__).resolve().parents[2]
SUITE = Path('integration-tests/benchmark_suites/r3-short.yaml')
TRACE_SUITE = Path('integration-tests/benchmark_suites/r3-trace.yaml')
JAEGER = 'jaegertracing/all-in-one@sha256:968b31672178558da8a0c1bfc86336647d756ee6997ca196698796f5262adef1'
TRACE_FILTER = ('info,golem_worker_executor::worker::durable_stream_producer=debug,'
                'golem_worker_executor::services::stream_session_index=debug,'
                'golem_worker_executor::durable_host::rpc=debug,'
                'golem_worker_executor::durable_host::durable_session=debug')
REQUIRED = {
    'latency-small': ['cold-invocation', 'hot-invocation'],
    'throughput-cpu-intensive': ['rust-agent-invocation', 'ts-agent-invocation'],
    'throughput-large-input': ['ts-agent-rpc-remote-invocation'],
    'streaming-rpc-history': ['guest-time-to-first-chunk'],
}


def save(path, value):
    with path.open('x') as stream:
        json.dump(value, stream, indent=2)


def remaining(deadline):
    seconds = deadline - time.time() - 120
    if seconds <= 0:
        raise RuntimeError('The immutable study deadline reached its cleanup reserve')
    return seconds


def checked(command, deadline, limit=120):
    return subprocess.check_output(command, cwd=ROOT, text=True,
                                   timeout=min(limit, remaining(deadline)))


def validate(path, run_id, runner, suite_path=SUITE):
    collection = json.loads(path.read_text())
    if len(collection['runs']) != 1:
        raise ValueError('Expected one diagnostic record')
    run = collection['runs'][0]
    if (run['run_id'] != run_id or run['source']['commitSha'] != SOURCE
            or run['runner']['id'] != runner or run['schema_version'] != 2
            or run['harness_version'] != 1):
        raise ValueError('Unexpected run identity')
    suite = yaml.safe_load((ROOT / suite_path).read_text())
    fingerprint = hashlib.sha256(json.dumps(suite, separators=(',', ':')).encode()).hexdigest()
    if run['suite_fingerprint'] != fingerprint:
        raise ValueError('Unexpected suite fingerprint')
    expected = {item['name']: item for item in suite['benchmarks']}
    if len(run['results']) != len(expected) or {e['name'] for e in run['results']} != set(expected):
        raise ValueError('Incorrect workload coverage')
    for entry in run['results']:
        item = expected[entry['name']]
        environment = entry['measurement_environment']
        identity = [1, item['version'], runner, environment['hardware_fingerprint'],
                    environment['storage_profile'], False]
        key = hashlib.sha256(json.dumps(identity, separators=(',', ':')).encode()).hexdigest()
        if (entry['series_key'] != key or environment['runner_id'] != runner
                or environment['sockets_enabled']
                or environment['storage_profile'] != 'postgres-kv-indexed-scheduler+filesystem-blob+sqlite-shard'
                or entry['iterations'] != 3
                or entry['entry_version'] != item['version'] or len(entry['results']) != 1):
            raise ValueError('Incorrect series or iteration identity')
        config = entry['results'][0]
        expected_config = dict(clusterSize=item['clusterSize'][0], size=item['size'][0],
                               length=item['length'][0], disableCompilationCache=False)
        if config['run_config'] != expected_config or config.get('failures'):
            raise ValueError('Incorrect or failed configuration')
        iterations = config['iteration_results']
        if [it['index'] for it in iterations] != [0, 1, 2]:
            raise ValueError('Missing iteration positions')
        for it in iterations:
            if it.get('skipped') or it['failures']:
                raise ValueError('Failed or skipped iteration')
            for metric in REQUIRED[entry['name']]:
                samples = it['duration_samples'].get(metric, [])
                if not samples or any(not math.isfinite(v) or v <= 0 for v in samples):
                    raise ValueError(f'Missing or invalid required duration: {metric}')
    artifacts = run['artifacts']
    services = {'golem-component-compilation-service', 'golem-registry-service',
                'golem-shard-manager', 'golem-worker-executor', 'golem-worker-service'}
    fixtures = {'benchmark_agent_rust_release', 'golem_it_agent_rpc_rust_release'}
    if suite_path == SUITE:
        fixtures.add('benchmark_agent_ts')
    if not artifacts['runner'] or set(artifacts['services']) != services or set(artifacts['fixtures']) != fixtures:
        raise ValueError('Missing artifact provenance')
    manifest = json.loads((path.parent / 'bundle-members.json').read_text())
    records = [(artifacts['runner'], 'target/benchmarks/benchmarks')]
    records += [(record, 'target/release/' + ('worker-executor' if name == 'golem-worker-executor' else name))
                for name, record in artifacts['services'].items()]
    records += [(record, f'test-components/{name}.wasm') for name, record in artifacts['fixtures'].items()]
    for artifact, member in records:
        installed = ROOT / member
        if Path(artifact['path']).resolve() != installed.resolve():
            raise ValueError('Artifact path differs from the verified bundle')
        actual = hashlib.file_digest(installed.open('rb'), 'sha256').hexdigest()
        if actual != artifact['sha256'] or actual != manifest[member]:
            raise ValueError('Artifact differs from the verified bundle')
    return run


def preflight(args):
    deadline = min(args.deadline, time.time() + 720)
    suite_path = TRACE_SUITE if getattr(args, 'trace_diagnostic', False) else SUITE
    if hashlib.file_digest(args.archive.open('rb'), 'sha256').hexdigest() != BUNDLE_SHA:
        raise ValueError('Archive substitution rejected')
    remaining(args.deadline)
    manifest = {}
    with tarfile.open(args.archive) as archive:
        archive.extractall(ROOT, filter='data')
        for member in archive:
            if member.isfile():
                installed = ROOT / member.name
                digest = hashlib.file_digest(archive.extractfile(member), 'sha256').hexdigest()
                manifest[Path(member.name).as_posix()] = digest
                if hashlib.file_digest(installed.open('rb'), 'sha256').hexdigest() != digest:
                    raise ValueError('Installed bundle member differs')
                if bool(installed.stat().st_mode & 0o111) != bool(member.mode & 0o111):
                    raise ValueError('Executable permission differs')
    manifest_path = args.output / 'bundle-members.json'
    if manifest_path.exists():
        if json.loads(manifest_path.read_text()) != manifest:
            raise ValueError('Verified bundle manifest changed')
    else:
        save(manifest_path, manifest)
    inventory = json.loads((ROOT / 'builtin-artifacts.json').read_text())
    for name, item in inventory.items():
        os.environ[f'GOLEM__BUILTIN_ARTIFACTS__SOURCE_OVERRIDES__{name.upper()}__URL'] = (
            (ROOT / item['path']).as_uri())
    os.environ['GOLEM_BENCH_SOCKETS_ENABLED'] = 'false'
    redis = checked(['redis-server', '--version'], deadline)
    if 'v=8.0.2 ' not in redis:
        raise ValueError(f'Expected Redis 8.0.2, got {redis.strip()}')
    checked(['docker', 'pull', POSTGRES], deadline, 300)
    checked(['docker', 'tag', POSTGRES, 'postgres:17.7'], deadline)
    image = checked(['docker', 'image', 'inspect', 'postgres:17.7', '--format', '{{.Id}}'], deadline).strip()
    if image != checked(['docker', 'image', 'inspect', POSTGRES, '--format', '{{.Id}}'], deadline).strip():
        raise ValueError('PostgreSQL image differs')
    checked(['target/benchmarks/benchmarks', 'suite', '--check-artifacts', str(suite_path), 'spawned'], deadline)
    save(args.output / f'preflight-{args.first}.json', dict(
        source=SOURCE, bundle_sha256=BUNDLE_SHA, postgres=image, redis=redis.strip(),
        orchestration=checked(['git', 'rev-parse', 'HEAD'], deadline).strip(),
        suite_sha256=hashlib.sha256((ROOT / suite_path).read_bytes()).hexdigest(),
        deadline=args.deadline, runner=args.runner, host=args.host, cohort=args.cohort,
        uname=checked(['uname', '-a'], deadline),
        cpu=checked(['lscpu'], deadline),
        memory=checked(['free', '-b'], deadline),
        docker=checked(['docker', 'version'], deadline)))
    return image


def execute_round(args, index, image):
    remaining(args.deadline)
    diagnostic = getattr(args, 'trace_diagnostic', False)
    suite_path = TRACE_SUITE if diagnostic else SUITE
    traced = diagnostic and ((args.host in ('1', '3')) == (index == 2))
    run_id = f'{args.cohort}-{args.host}-{index}'
    prefix = args.output / f'round-{index}'
    prefix.with_suffix('.started').mkdir()
    result_path = prefix.with_suffix('.json')
    if result_path.exists() or prefix.with_suffix('.status.json').exists():
        raise FileExistsError('Refusing to overwrite an earlier attempt')
    command = ['target/benchmarks/benchmarks', '--primary-only', '--retain-details',
               '--retain-selected-zero-counts', 'suite', '--run-id', run_id,
               '--save-to-json', str(result_path), '--runner-id', args.runner,
               '--source-repository', 'golemcloud/golem', '--source-commit-sha', SOURCE,
               '--source-ref', 'refs/heads/gol-1191-benchmark-iterations', str(suite_path),
               'spawned', '--registry-service-http-port', '18081']
    environment = os.environ.copy()
    if diagnostic:
        environment['GOLEM__TRACING__OTLP__ENABLED'] = 'false'
        environment['GOLEM_OTLP_FILTER'] = TRACE_FILTER
        environment['OTEL_BSP_MAX_QUEUE_SIZE'] = '262144'
        if traced:
            command.insert(1, '--otlp')
    started, monotonic = time.time(), time.monotonic()
    original = set(checked(['docker', 'ps', '-aq'], args.deadline).split())
    status = dict(started=started, run_id=run_id, host=args.host, valid=False)
    if diagnostic:
        status.update(traced=traced, suite=str(suite_path), command=command,
                      otlp_filter=TRACE_FILTER, collector=JAEGER)
    process = None
    try:
        with prefix.with_suffix('.log').open('x') as log:
            process = subprocess.Popen(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                       start_new_session=True, env=environment)
            if diagnostic:
                limit = min(time.monotonic() + 480, time.monotonic() + remaining(args.deadline))
                with prefix.with_suffix('.resources.jsonl').open('x') as resources:
                    while process.poll() is None:
                        resources.write(json.dumps(resource_sample()) + '\n')
                        resources.flush()
                        timeout = min(2, limit - time.monotonic())
                        if timeout <= 0:
                            raise subprocess.TimeoutExpired(command, 480)
                        try:
                            process.wait(timeout=timeout)
                        except subprocess.TimeoutExpired:
                            pass
                status['exit_code'] = process.returncode
            else:
                status['exit_code'] = process.wait(timeout=min(480, remaining(args.deadline)))
        wall = time.time() - started
        elapsed = time.monotonic() - monotonic
        status.update(wall_seconds=wall, monotonic_seconds=elapsed)
        if abs(wall - elapsed) > 5 or wall > 480 or time.time() >= args.deadline - 120:
            raise RuntimeError('Interrupted or expired observation')
        if status['exit_code'] != 0:
            raise RuntimeError('Benchmark command failed')
        run = validate(result_path, run_id, args.runner, suite_path)
        if index > 1:
            first = validate(args.output / 'round-1.json', f'{args.cohort}-{args.host}-1', args.runner, suite_path)
            if [(e['name'], e['series_key']) for e in run['results']] != [(e['name'], e['series_key']) for e in first['results']]:
                raise ValueError('Hardware or series fragmentation between rounds')
        current_image = checked(['docker', 'image', 'inspect', 'postgres:17.7', '--format', '{{.Id}}'], args.deadline).strip()
        if current_image != image:
            raise ValueError('PostgreSQL image changed during measurement')
        status['valid'] = True
    except BaseException as error:
        status['error'] = str(error)
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
                    if sig == signal.SIGTERM:
                        time.sleep(0.2)
            # Only stop new PostgreSQL fixtures on Golem test networks; retain their data.
            containers = subprocess.check_output(['docker', 'ps', '-q', '--filter', 'ancestor=postgres:17.7'],
                                                 text=True, timeout=10).split()
            for container in set(containers) - original:
                networks = subprocess.check_output(['docker', 'inspect', container, '--format',
                                                    '{{json .NetworkSettings.Networks}}'], text=True, timeout=10)
                if any(name.startswith('golem_test_network_') for name in json.loads(networks)):
                    subprocess.run(['docker', 'stop', '-t', '3', container], check=True, timeout=10)
        except BaseException as error:
            status.update(valid=False, cleanup_error=str(error))
            raise
        finally:
            status['ended'] = time.time()
            try:
                save(prefix.with_suffix('.status.json'), status)
            finally:
                for sig, handler in handlers.items():
                    signal.signal(sig, handler)
    print(f'PASS {run_id}', flush=True)


def resource_sample():
    sample = dict(timestamp=time.time(), monotonic=time.monotonic(), system={}, processes={})
    def read(path):
        try:
            return path.read_text()
        except OSError as error:
            return {'unavailable': str(error)}
    for name in ['stat', 'loadavg', 'meminfo', 'diskstats', 'pressure/cpu', 'pressure/io', 'pressure/memory']:
        sample['system'][name] = read(Path('/proc') / name)
    for pid in Path('/proc').glob('[0-9]*'):
        try:
            name = (pid / 'comm').read_text().strip()
        except OSError:
            continue
        if name in {'benchmarks', 'worker-executor', 'postgres', 'redis-server', 'golem-worker-se',
                    'golem-registry-', 'golem-shard-man', 'golem-component', 'all-in-one-linu'}:
            entry = dict(name=name, **{k: read(pid / k) for k in ['stat', 'io', 'status', 'cgroup']})
            group = entry['cgroup']
            if isinstance(group, str):
                for line in group.splitlines():
                    if line.startswith('0::'):
                        cgroup = Path('/sys/fs/cgroup') / line[3:].lstrip('/')
                        entry['cpu.stat'] = read(cgroup / 'cpu.stat')
                        entry['cpu.max'] = read(cgroup / 'cpu.max')
            sample['processes'][pid.name] = entry
    return sample


def jaeger_json(path, timeout=10):
    with urllib.request.urlopen(f'http://127.0.0.1:16686/api/{path}', timeout=timeout) as response:
        return json.load(response)


def trace_diagnostic(args, image):
    """Keep one collector for both arms and preserve its database even on failure."""
    name = f'benchmark-trace-{args.host}'
    database = args.output / 'jaeger-database'
    database.mkdir()
    attempted = False
    started = time.time()
    try:
        checked(['docker', 'pull', JAEGER], args.deadline, 300)
        attempted = True
        receipt = checked(['docker', 'run', '-d', '--name', name, '--user', f'{os.getuid()}:{os.getgid()}',
                           '-p', '127.0.0.1:4318:4318', '-p', '127.0.0.1:16686:16686',
                           '-e', 'COLLECTOR_OTLP_ENABLED=true', '-e', 'SPAN_STORAGE_TYPE=badger',
                           '-e', 'BADGER_EPHEMERAL=false', '-e', 'BADGER_DIRECTORY_VALUE=/badger/data',
                           '-e', 'BADGER_DIRECTORY_KEY=/badger/key',
                           '-v', f'{database}:/badger', JAEGER], args.deadline)
        save(args.output / 'collector.json', dict(image=JAEGER, container=receipt.strip(), started=started,
             filter=TRACE_FILTER, host=args.host, order=['off', 'on'] if args.host in ('1', '3') else ['on', 'off']))
        for _ in range(30):
            remaining(args.deadline)
            try:
                jaeger_json('services', timeout=2)
                break
            except OSError:
                time.sleep(1)
        else:
            raise RuntimeError('Jaeger did not become ready')
        for index in (1, 2):
            execute_round(args, index, image)
    finally:
        if attempted:
            # Export after the service processes exit; Badger remains a second, unfiltered receipt.
            export = dict(started=started, ended=time.time(), services=[], errors=[])
            try:
                export_deadline = min(args.deadline - 30, time.time() + 60)
                services = jaeger_json('services')['data']
                export['services'] = services
                for service in services:
                    budget = export_deadline - time.time()
                    if budget <= 0:
                        raise TimeoutError('Trace export budget expired; retain the collector database')
                    query = urllib.parse.urlencode(dict(service=service, start=int(started*1e6),
                        end=int(time.time()*1e6), limit=50000))
                    data = jaeger_json(f'traces?{query}', timeout=min(20, budget))
                    filename = f'traces-{hashlib.sha256(service.encode()).hexdigest()[:12]}.json'
                    save(args.output / filename, data)
                    export.setdefault('files', []).append(dict(service=service, file=filename,
                        traces=len(data.get('data', [])), capped=len(data.get('data', [])) >= 50000))
            except Exception as error:
                export['errors'].append(str(error))
            finally:
                try:
                    try:
                        with (args.output / 'collector.log').open('x') as log:
                            subprocess.run(['docker', 'logs', name], stdout=log, stderr=subprocess.STDOUT, timeout=10, check=True)
                    finally:
                        subprocess.run(['docker', 'stop', '-t', '5', name], timeout=15, check=True)
                finally:
                    save(args.output / 'trace-export.json', export)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--deadline', type=int, required=True)
    parser.add_argument('--cohort', required=True)
    parser.add_argument('--runner', required=True, choices=['amp-orb-a1.xxlarge', 'github-actions-blacksmith-32vcpu'])
    parser.add_argument('--host', required=True, choices=['orb', '1', '2', '3', '4'])
    parser.add_argument('--first', type=int, default=1)
    parser.add_argument('--last', type=int, default=5)
    parser.add_argument('--trace-diagnostic', action='store_true')
    args = parser.parse_args()
    if args.trace_diagnostic and (args.host == 'orb' or args.first != 1 or args.last != 2
                                  or args.deadline > time.time() + 1800):
        parser.error('Trace diagnostic requires a CI host, exactly two arms and a 30-minute deadline')
    if not 1 <= args.first <= args.last <= 5:
        parser.error('Only numbered attempts 1 through 5 are allowed')
    if args.deadline > time.time() + 3600:
        parser.error('Deadline cannot exceed one hour from now')
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / f'batch-{args.first}-{args.last}.started').mkdir()
    if args.first > 1:
        original = json.loads((args.output / 'preflight-1.json').read_text())
        if any(original[key] != getattr(args, key) for key in ('deadline', 'runner', 'host', 'cohort')):
            raise ValueError('Continuation changed the original deadline or cohort')
    def interrupted(signum, frame):
        raise InterruptedError(f'Received signal {signum}')
    signal.signal(signal.SIGTERM, interrupted)
    for previous in range(1, args.first):
        status = json.loads((args.output / f'round-{previous}.status.json').read_text())
        if not status['valid']:
            raise ValueError('Preceding attempt was not successful')
        validate(args.output / f'round-{previous}.json', f'{args.cohort}-{args.host}-{previous}', args.runner)
    image = preflight(args)
    if args.trace_diagnostic:
        trace_diagnostic(args, image)
    else:
        for index in range(args.first, args.last + 1):
            execute_round(args, index, image)


if __name__ == '__main__':
    main()
