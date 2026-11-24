#!/usr/bin/env python3
"""One bounded selector validation with original runtime bytes and a new harness."""
import argparse
import collections
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import tarfile
import time

import r3_coverage as original
import validate_r3_metrics as metrics

ROOT = Path(__file__).resolve().parents[2]
SUITE = Path(__file__).with_name('r3-selector.yaml')
require = metrics.require
save = original.save
sha = original.sha


def declaration(source):
    require(re.fullmatch('[0-9a-f]{40}', source), 'Expected full harness source SHA')
    suite = json.loads(SUITE.read_text())
    configurations = []
    for name, size, length in original.CASES:
        config = dict(clusterSize=5, size=size, length=length, disableCompilationCache=False)
        configurations.append(dict(entry=name, config=config, iterations=5, version=2,
                                   **metrics.metric_rules(name, config)))
    return dict(source=source, suite_name=suite['name'],
                suite_fingerprint=metrics.digest(json.dumps(suite, separators=(',', ':')).encode()),
                fixtures=['benchmark_agent_rust_release', 'benchmark_agent_ts'], configurations=configurations)


def analyze(run, source):
    declared = declaration(source)
    configurations, _ = metrics.validate_identity(run, declared)
    rows = []
    for spec in declared['configurations']:
        raw = configurations[metrics.configuration_key(spec['entry'], spec['config'])]
        for it in raw['iteration_results']:
            view = metrics.iteration_view(it, spec, set())
            require(view['valid'], f'Invalid iteration: {view.get("reason")}')
            for language in ('rust', 'ts'):
                for target in range(spec['config']['size']):
                    route = 'local' if it['index'] % 2 == target % 2 else 'remote'
                    key = f'{language}-agent-rpc-{route}-worker-{target}__secondary'
                    require(len(it['duration_samples'].get(key, [])) == metrics.CALLS[spec['entry']],
                            f'Wrong selected route: {spec["entry"]}/{it["index"]}/{language}/{target}')
        for language in ('rust', 'ts'):
            for route in ('local', 'remote'):
                indexes = [it['index'] for it in raw['iteration_results']
                           if it['duration_samples'].get(f'{language}-agent-rpc-{route}-invocation')]
                require(bool(indexes), 'Missing possible route')
                rows.append(dict(entry=spec['entry'], config=spec['config'], language=language,
                                 route=route, applicable_indexes=indexes))
    return dict(run_id=run['run_id'], rows=rows, coverage_complete=True,
                r3_acceptance=False, n_accepted=False,
                qualification='Exact selector schedule only; four hosts are not full-suite R3 replication')


def validate_origin(origin):
    require(origin['id'] == 44078542455 and origin['head_sha'] == metrics.SOURCE
            and origin['path'] == '.github/workflows/benchmark.yaml'
            and origin['event'] == 'workflow_dispatch' and origin['run_attempt'] == 1
            and origin['conclusion'] == 'success', 'Wrong original producer workflow')


def install_overlay(path, root, members, source, run_id):
    with tarfile.open(path) as archive:
        items = archive.getmembers()
        require(len(items) == 2 and {item.name for item in items} == {'benchmarks', 'harness-provenance.json'}
                and all(item.isfile() for item in items), 'Unsafe or unexpected harness overlay members')
        provenance = json.load(archive.extractfile('harness-provenance.json'))
        require(provenance['source'] == source and provenance['run_id'] == run_id
                and provenance['attempt'] == '1' and provenance['base_source'] == metrics.SOURCE
                and provenance['base_bundle_sha256'] == original.BUNDLE_SHA, 'Wrong overlay identity')
        executable = archive.getmember('benchmarks')
        digest = hashlib.file_digest(archive.extractfile(executable), 'sha256').hexdigest()
        require(digest == provenance['sha256'] and executable.mode & 0o100, 'Wrong overlay payload')
        member = 'target/benchmarks/benchmarks'
        with archive.extractfile(executable) as payload, (root / member).open('wb') as out:
            shutil.copyfileobj(payload, out)
        (root / member).chmod(0o755)
        require(sha(root / member) == digest, 'Installed harness hash differs')
        members[member] = digest
    return provenance


def selections(log):
    receipts = []
    for line in log.splitlines():
        line = re.sub(r'(?:\x1b|\\u001b)\[[0-9;]*m', '', line)
        match = re.search(r'rpc_selection(?:=|:\s*)', line)
        if match:
            receipt, _ = json.JSONDecoder().raw_decode(line[match.end():])
            receipts.append(receipt)
    return receipts


def validate_selections(receipts):
    groups = collections.defaultdict(list)
    for item in receipts:
        require(type(item['iteration']) is int and item['iteration'] in range(5)
                and type(item['target']) is int and 0 <= item['target'] < 10
                and type(item['candidate']) is int and 0 <= item['candidate'] < 10000,
                'Invalid selection receipt')
        local = item['iteration'] % 2 == item['target'] % 2
        require(item['local'] is local and (item['parent_pod'] == item['child_pod']) == local,
                'Selection receipt disagrees with route schedule')
        require(item['parent'] != item['child'], 'Caller/callee identity collision')
        groups[item['component_id']].append(item)
    require(len(groups) == 40, 'Missing or duplicate component selection groups')
    sizes = collections.Counter()
    for group in groups.values():
        size = len(group)
        require(size in (1, 10) and {item['target'] for item in group} == set(range(size))
                and len({item['iteration'] for item in group}) == 1, 'Incomplete selection targets')
        sizes[(group[0]['iteration'], size)] += 1
    require(sizes == collections.Counter({(i, size): count for i in range(5)
                                          for size, count in ((1, 6), (10, 2))}), 'Wrong selection inventory')


def checked(command, root, deadline, limit=120):
    remaining = deadline - time.time() - 120
    require(remaining > 0, 'Immutable deadline cleanup reserve reached')
    return subprocess.check_output(command, cwd=root, text=True, timeout=min(limit, remaining))


def freeze(args):
    args.output.mkdir(parents=True, exist_ok=False)
    validate_origin(json.loads(args.origin.read_text()))
    require(sha(args.archive) == original.BUNDLE_SHA, 'Original archive hash mismatch')
    # Keep bytes, not a newly created tar archive with different metadata.
    shutil.copyfile(args.archive, args.output / 'benchmark-bundle.tar.gz')
    require(sha(args.output / 'benchmark-bundle.tar.gz') == original.BUNDLE_SHA, 'Frozen archive differs')
    shutil.copyfile(args.origin, args.output / 'original-origin.json')


def package(args):
    require(sha(args.output / 'benchmark-bundle.tar.gz') == original.BUNDLE_SHA, 'Frozen archive differs')
    require(os.access(args.binary, os.X_OK), 'Harness is not executable')
    provenance = dict(source=args.source, run_id=args.run_id, attempt='1',
                      base_source=metrics.SOURCE, base_bundle_sha256=original.BUNDLE_SHA,
                      sha256=sha(args.binary), lock_sha256=sha(ROOT / 'Cargo.lock'),
                      suite_sha256=sha(SUITE), analyzer_sha256=sha(Path(__file__)),
                      metric_rules_sha256=sha(Path(metrics.__file__)),
                      command='cargo build --locked --profile benchmarks -p integration-tests --bin benchmarks')
    save(args.output / 'harness-provenance.json', provenance)
    with tarfile.open(args.output / 'harness-overlay.tar.gz', 'w:gz') as archive:
        archive.add(args.binary, arcname='benchmarks')
        archive.add(args.output / 'harness-provenance.json', arcname='harness-provenance.json')
    save(args.output / 'preparation-receipt.json', dict(provenance=provenance,
         archive_sha256=original.BUNDLE_SHA, overlay_sha256=sha(args.output / 'harness-overlay.tar.gz')))


def execute(args):
    require(os.environ.get('GITHUB_ACTIONS') == 'true' and os.environ['GITHUB_RUN_ATTEMPT'] == '1',
            'One fresh GitHub Actions attempt required')
    require(os.environ['GITHUB_SHA'] == args.source, 'Orchestration source differs')
    args.output.mkdir(parents=True, exist_ok=False)
    status = dict(valid=False, started=time.time(), deadline=args.deadline, publication=False)
    process = None
    def interrupted(signum, frame):
        raise InterruptedError(f'Interrupted by signal {signum}')
    handlers = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGTERM, signal.SIGINT)}
    try:
        require(checked(['git', 'rev-parse', 'HEAD'], args.root, args.deadline).strip() == metrics.SOURCE,
                'Wrong runtime configuration checkout')
        validate_origin(json.loads((args.prepared / 'original-origin.json').read_text()))
        original_members = original.install(args.prepared / 'benchmark-bundle.tar.gz', args.root)
        save(args.output / 'original-members.json', original_members)
        members = dict(original_members)
        receipt = json.loads((args.prepared / 'preparation-receipt.json').read_text())
        require(receipt['archive_sha256'] == original.BUNDLE_SHA
                and receipt['overlay_sha256'] == sha(args.prepared / 'harness-overlay.tar.gz'), 'Preparation receipt differs')
        provenance = install_overlay(args.prepared / 'harness-overlay.tar.gz', args.root, members,
                                     args.source, args.run_id)
        require(provenance == receipt['provenance'] and provenance['suite_sha256'] == sha(SUITE)
                and provenance['analyzer_sha256'] == sha(Path(__file__))
                and provenance['metric_rules_sha256'] == sha(Path(metrics.__file__)), 'Prepared contract differs')
        save(args.output / 'harness-provenance.json', provenance)
        save(args.output / 'installed-members.json', members)
        save(args.output / 'artifact-sources.json', {
            name: args.source if name == 'target/benchmarks/benchmarks' else metrics.SOURCE for name in members})
        require(not checked(['docker', 'ps', '-aq'], args.root, args.deadline).split(), 'Runner is not fresh')
        require('v=8.0.2 ' in checked(['redis-server', '--version'], args.root, args.deadline), 'Wrong Redis')
        checked(['docker', 'pull', original.POSTGRES], args.root, args.deadline, 300)
        checked(['docker', 'tag', original.POSTGRES, 'postgres:17.7'], args.root, args.deadline)
        harness = args.root / 'target/benchmarks/benchmarks'
        checked([str(harness), 'suite', '--check-artifacts', str(SUITE), 'spawned'], args.root, args.deadline)
        run_id = f'selector-{args.run_id}-{args.host}'
        command = [str(harness), '--primary-only', '--retain-details', '--retain-selected-zero-counts',
                   'suite', '--run-id', run_id, '--save-to-json', str(args.output / 'raw.json'),
                   '--runner-id', metrics.RUNNER, '--source-repository', 'golemcloud/golem',
                   '--source-commit-sha', args.source, '--source-ref', args.source, str(SUITE), 'spawned',
                   '--workspace-root', str(args.root), '--component-directory', str(args.root / 'test-components'),
                   '--registry-service-http-port', '18081']
        environment = dict(os.environ, GOLEM_BENCH_SOCKETS_ENABLED='false', GOLEM__TRACING__OTLP__ENABLED='false')
        for name, item in json.loads((args.root / 'builtin-artifacts.json').read_text()).items():
            environment[f'GOLEM__BUILTIN_ARTIFACTS__SOURCE_OVERRIDES__{name.upper()}__URL'] = (args.root / item['path']).as_uri()
        save(args.output / 'plan.json', dict(command=command, source=args.source, runtime_source=metrics.SOURCE,
             deadline=args.deadline, max_process_seconds=900, publication=False))
        save(args.output / 'manifest.json', declaration(args.source))
        cutoff = min(time.time() + 900, args.deadline - 120)
        log_path = args.output / 'process.log'
        with log_path.open('x') as log:
            status['measurement_started'] = time.time()
            require(time.time() < cutoff, 'Measurement budget exhausted before start')
            process = subprocess.Popen(command, cwd=args.root, env=environment, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            while process.poll() is None:
                require(time.time() < cutoff, 'Measurement budget exhausted')
                require(log_path.stat().st_size <= 134217728, 'Log budget exhausted')
                try:
                    process.wait(timeout=min(1, max(.01, cutoff - time.time())))
                except subprocess.TimeoutExpired:
                    pass
        status.update(measurement_ended=time.time(), exit_code=process.returncode)
        require(process.returncode == 0 and time.time() < cutoff, 'Process failed or exceeded budget; no replacement')
        require(log_path.stat().st_size <= 134217728, 'Log budget exhausted')
        collection = json.loads((args.output / 'raw.json').read_text())
        require(len(collection['runs']) == 1 and collection['runs'][0]['run_id'] == run_id, 'Wrong process identity')
        run = collection['runs'][0]
        for entry in run['results']:
            require(entry['measurement_environment']['hardware_fingerprint'] == original.HARDWARE, 'Wrong hardware class')
        records = [(run['artifacts']['runner'], 'target/benchmarks/benchmarks')]
        records += [(item, 'target/release/' + ('worker-executor' if name == 'golem-worker-executor' else name))
                    for name, item in run['artifacts']['services'].items()]
        records += [(item, f'test-components/{name}.wasm') for name, item in run['artifacts']['fixtures'].items()]
        for artifact, member in records:
            require(Path(artifact['path']).resolve() == args.root / member, 'Measured artifact path differs')
            require(artifact['sha256'] == members[member] == sha(args.root / member), 'Measured artifact differs')
        observed = selections(log_path.read_text())
        validate_selections(observed)
        save(args.output / 'selections.json', observed)
        save(args.output / 'coverage.json', analyze(run, args.source))
        save(args.output / 'raw-receipt.json', dict(sha256=sha(args.output / 'raw.json'), run_id=run_id))
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
            require(not remaining, 'Container remains; no progression, runner teardown owns cleanup')
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
    frozen = commands.add_parser('freeze')
    frozen.add_argument('--archive', type=Path, required=True)
    frozen.add_argument('--origin', type=Path, required=True)
    packed = commands.add_parser('package')
    packed.add_argument('--binary', type=Path, required=True)
    run = commands.add_parser('run')
    run.add_argument('--root', type=lambda s: Path(s).resolve(), required=True)
    run.add_argument('--prepared', type=Path, required=True)
    run.add_argument('--deadline', type=int, required=True)
    run.add_argument('--host', choices=('1', '2', '3', '4'), required=True)
    analysis = commands.add_parser('analyze')
    analysis.add_argument('record', type=Path)
    for command in (packed, run, analysis):
        command.add_argument('--source', required=True)
    for command in (packed, run):
        command.add_argument('--run-id', required=True)
    for command in (frozen, packed, run):
        command.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.mode == 'freeze':
        freeze(args)
    elif args.mode == 'package':
        package(args)
    elif args.mode == 'run':
        execute(args)
    else:
        collection = json.loads(args.record.read_text())
        require(len(collection['runs']) == 1, 'Expected one process')
        print(json.dumps(analyze(collection['runs'][0], args.source), indent=2, allow_nan=False))


if __name__ == '__main__':
    main()
