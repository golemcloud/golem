import { spawnSync } from 'node:child_process';
import { setProcessBackend } from 'wasm-rquickjs:child-process';
import { runJavaScript } from 'wasm-rquickjs:execution';

export function run(value) {
    const local = spawnSync(process.execPath, ['-p', '40 + 2'], { encoding: 'utf8' });
    if (local.status !== 0 || local.stdout.trim() !== '42') throw new Error('Local Node execution lost priority');
    if (spawnSync('not-handled').error?.code !== 'ENOSYS') throw new Error('Unknown command fallback changed');
    try {
        setProcessBackend({ spawnSync: 7 });
        throw new Error('Invalid backend was accepted');
    } catch (error) {
        if (!(error instanceof TypeError)) throw error;
    }
    const result = spawnSync('answer', [String(value)], { encoding: 'utf8' });
    if (result.status !== 0 || result.stderr !== '') throw new Error('Unexpected backend result');
    return Number(result.stdout);
}

export async function runJob(value) {
    setProcessBackend(null);
    if (spawnSync('answer').error?.code !== 'ENOSYS') throw new Error('Backend disable failed');
    globalThis.parentBackendMarker = true;
    const job = await runJavaScript({
        source: `
            if ('parentBackendMarker' in globalThis) throw new Error('Inherited parent globals');
            const { spawnSync } = await import('node:child_process');
            const result = spawnSync('answer', [${JSON.stringify(String(value))}], { encoding: 'utf8' });
            if (result.status !== 0 || result.stderr !== '') throw new Error('Fresh runtime backend missing');
            return Number(result.stdout);
        `,
        cwd: '/', timeoutMs: 3000, maxBytes: 1024,
    });
    if (job.stdout || job.stderr || job.overflowed) throw new Error('Nested output leaked');
    if (spawnSync('answer').error?.code !== 'ENOSYS') throw new Error('Child changed parent backend');
    return job.value;
}
