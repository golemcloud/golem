import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { isDeepStrictEqual } from 'node:util';

const [checkout, commit, runner, output] = process.argv.slice(2);
if (!checkout || !output || !/^[0-9a-f]{40}$/.test(commit) || !runner) {
    throw new Error('Expected results checkout, exact Golem commit, runner and evidence directory');
}
const data = path.resolve(checkout, 'public/data');
const index = JSON.parse(fs.readFileSync(path.join(data, 'index.json'), 'utf8'));
const matching = index.runs.filter(run => run.source?.repository === 'golemcloud/golem'
    && run.source?.commitSha === commit && run.runner?.id === runner && run.suite === 'CI');
function chronologicalTimestamp(timestamp) {
    const [seconds, fraction = ''] = timestamp.slice(0, -1).split('.');
    return `${seconds}.${fraction.replace(/0+$/, '')}`;
}
matching.sort((a, b) => chronologicalTimestamp(a.timestamp).localeCompare(chronologicalTimestamp(b.timestamp)));
const selected = matching.at(-1);
let analysis;
if (!selected) {
    analysis = { status: 'missing-ci-result', shouldAlert: false, commit, runner,
        message: 'No published CI run for this exact commit; no measurement or publication was started.' };
} else {
    if (!/^runs\/[0-9a-f]{24}\.json$/.test(selected.file)) {
        throw new Error('Published run has an invalid detail-file path');
    }
    const bytes = fs.readFileSync(path.join(data, selected.file));
    const run = JSON.parse(bytes);
    const { validateIncomingCollection, storedRun } = await import(pathToFileURL(path.resolve(checkout, 'scripts/append-results.mjs')));
    validateIncomingCollection({ runs: [run] });
    if (run.source?.repository !== 'golemcloud/golem' || run.source?.commitSha !== commit
        || run.runner?.id !== runner || run.suite !== 'CI' || run.timestamp !== selected.timestamp) {
        throw new Error('Published detail record does not match the selected CI run');
    }
    if (!isDeepStrictEqual(storedRun(run).history, selected)) {
        throw new Error('Published index measurements do not match the retained detail record');
    }
    const { analyzeRegressions } = await import(pathToFileURL(path.resolve(checkout, 'scripts/analyze-regressions.mjs')));
    analysis = analyzeRegressions(index, { runner, suite: 'CI', timestamp: selected.timestamp });
    if (analysis.latest?.commitSha !== commit || analysis.latest?.timestamp !== selected.timestamp) {
        throw new Error('Analysis did not select the exact published CI run');
    }
    fs.writeFileSync(path.join(output, 'selection.json'), JSON.stringify({
        commit, runner, timestamp: selected.timestamp, run_id: run.run_id,
        file: selected.file, sha256: crypto.createHash('sha256').update(bytes).digest('hex'),
        failures: run.results.flatMap(entry => entry.results.flatMap(result =>
            Object.entries(result.failures ?? {}).map(([metric, failure]) =>
                ({ entry: entry.name, run_config: result.run_config, metric, ...failure })))),
    }, null, 2), { flag: 'wx' });
}
fs.writeFileSync(path.join(output, 'analysis.json'), JSON.stringify(analysis, null, 2), { flag: 'wx' });
process.stdout.write(`${JSON.stringify(analysis, null, 2)}\n`);
