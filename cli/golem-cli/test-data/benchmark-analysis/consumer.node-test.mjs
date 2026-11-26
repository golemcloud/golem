import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../..');
const consumer = path.join(root, 'integration-tests/benchmark_suites/analyze_ci_benchmarks.mjs');
const commit = 'a'.repeat(40);
const runner = 'github-actions-blacksmith-32vcpu';
const file = `runs/${'b'.repeat(24)}.json`;
const selected = { schema_version: 2, suite: 'CI', timestamp: '2026-11-23T02:00:00Z',
    runner: { id: runner }, source: { repository: 'golemcloud/golem', commitSha: commit }, file,
    results: [{ name: 'cold', results: [{ failures: { invocation: { count: 1 } } }] }] };

function fixture(t, runs) {
    const scratch = path.join(root, '.amp/in/orb-backup');
    fs.mkdirSync(scratch, { recursive: true });
    const folder = fs.mkdtempSync(path.join(scratch, 'ci-analysis-test-'));
    t.after(() => fs.rmSync(folder, { recursive: true, force: true }));
    for (const name of ['public/data/runs', 'scripts', 'output']) fs.mkdirSync(path.join(folder, name), { recursive: true });
    fs.writeFileSync(path.join(folder, 'public/data/index.json'), JSON.stringify({ runs }));
    fs.writeFileSync(path.join(folder, `public/data/${file}`), JSON.stringify(selected));
    return folder;
}

function reader(folder) {
    // Model the separate reader repository's public module API, not its regression policy.
    fs.writeFileSync(path.join(folder, 'scripts/append-results.mjs'), `export function validateIncomingCollection(c) {
        if (c.runs[0].schema_version !== 2) throw Error('invalid record');
    }
    export function storedRun(run) { return {history: run}; }`);
    fs.writeFileSync(path.join(folder, 'scripts/analyze-regressions.mjs'), `export function analyzeRegressions(index, options) {
        const run = index.runs.find(r => r.runner.id === options.runner && r.suite === options.suite && r.timestamp === options.timestamp);
        return {status: 'insufficient-history', shouldAlert: false, latest: {...run.source, timestamp: run.timestamp}};
    }`);
}

function execute(folder) {
    return spawnSync(process.execPath, [consumer, folder, commit, runner, path.join(folder, 'output')], { encoding: 'utf8' });
}

test('selects the exact CI commit rather than a newer commit or historical orb run and retains failures', t => {
    const folder = fixture(t, [
        { ...selected, timestamp: '2026-11-24T02:00:00Z', source: { ...selected.source, commitSha: 'c'.repeat(40) } },
        { ...selected, timestamp: '2026-11-24T03:00:00Z', runner: { id: 'amp-orb-a1.xxlarge' } }, selected,
    ]);
    reader(folder);
    const result = execute(folder);
    assert.equal(result.status, 0, result.stderr);
    const receipt = JSON.parse(fs.readFileSync(path.join(folder, 'output/selection.json')));
    assert.equal(receipt.commit, commit);
    assert.equal(receipt.timestamp, selected.timestamp);
    assert.equal(receipt.failures[0].count, 1);
    assert.equal(JSON.parse(fs.readFileSync(path.join(folder, 'output/analysis.json'))).latest.commitSha, commit);
    assert.notEqual(execute(folder).status, 0, 'existing evidence must not be overwritten');
});

test('a missing exact result reports missing without importing an analyzer or measuring', t => {
    const folder = fixture(t, [{ ...selected, source: { ...selected.source, commitSha: 'c'.repeat(40) } }]);
    const result = execute(folder);
    assert.equal(result.status, 0, result.stderr);
    const analysis = JSON.parse(result.stdout);
    assert.equal(analysis.status, 'missing-ci-result');
    assert.equal(analysis.shouldAlert, false);
    assert.equal(fs.existsSync(path.join(folder, 'output/selection.json')), false);
});

test('selects the newest timestamp across second and sub-millisecond precision', t => {
    for (const timestamp of ['2026-11-23T02:00:00.100Z', '2026-11-23T02:00:00.000000001Z']) {
        const newest = { ...selected, timestamp };
        const folder = fixture(t, [newest, selected]);
        reader(folder);
        fs.writeFileSync(path.join(folder, `public/data/${file}`), JSON.stringify(newest));
        const result = execute(folder);
        assert.equal(result.status, 0, result.stderr);
        assert.equal(JSON.parse(result.stdout).latest.timestamp, timestamp);
    }
});

test('rejects index measurements that differ from the retained detail', t => {
    const folder = fixture(t, [{ ...selected, results: [] }]);
    reader(folder);
    assert.notEqual(execute(folder).status, 0);
    assert.equal(fs.existsSync(path.join(folder, 'output/analysis.json')), false);
});

test('rejects a mismatched detail source, invalid schema and unsafe detail path', t => {
    for (const mutation of ['source', 'schema', 'path']) {
        const folder = fixture(t, [mutation === 'path' ? { ...selected, file: '../outside.json' } : selected]);
        reader(folder);
        if (mutation !== 'path') {
            const bad = structuredClone(selected);
            if (mutation === 'source') bad.source.commitSha = 'c'.repeat(40);
            else bad.schema_version = 1;
            fs.writeFileSync(path.join(folder, `public/data/${file}`), JSON.stringify(bad));
        }
        assert.notEqual(execute(folder).status, 0, mutation);
        assert.equal(fs.existsSync(path.join(folder, 'output/analysis.json')), false);
    }
});

test('the shell entry point clones a snapshot and has no measurement or publishing fallback', t => {
    const folder = fixture(t, []);
    execFileSync('git', ['init', '--quiet', folder]);
    execFileSync('git', ['-C', folder, 'add', 'public']);
    execFileSync('git', ['-C', folder, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '-m', 'fixture']);
    const result = spawnSync('bash', [path.join(root, 'integration-tests/benchmark_suites/analyze_ci_benchmarks.sh')], {
        cwd: root, encoding: 'utf8', env: { ...process.env, BENCHMARK_RESULTS_REPOSITORY: folder,
            GOLEM_BENCHMARK_ARTIFACT_DIR: path.join(folder, 'artifacts') },
    });
    assert.equal(result.status, 0, result.stderr);
    const evidence = path.join(folder, 'artifacts', fs.readdirSync(path.join(folder, 'artifacts'))[0]);
    assert.equal(JSON.parse(fs.readFileSync(path.join(evidence, 'analysis.json'))).status, 'missing-ci-result');
    assert.equal(fs.readFileSync(path.join(evidence, 'results-commit.txt'), 'utf8').trim(), execFileSync('git', ['-C', folder, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim());
});
