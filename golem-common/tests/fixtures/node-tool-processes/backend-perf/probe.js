import { spawnSync } from 'node:child_process';
import { runJavaScript } from 'wasm-rquickjs:execution';

function localNode(count) {
    let total = 0;
    for (let index = 0; index < count; index++) {
        const result = spawnSync(process.execPath, ['-p', '21 * 2'], { encoding: 'utf8' });
        if (result.status !== 0 || result.stderr || result.stdout.trim() !== '42') {
            throw new Error('Local child process failed');
        }
        total++;
    }
    return total;
}

export function run(count) {
    return localNode(count);
}

export async function runJob(count) {
    const result = await runJavaScript({
        source: `const { spawnSync } = await import('node:child_process'); return (${localNode.toString()})(${count});`,
        cwd: '/', timeoutMs: 30000, maxBytes: 1024,
    });
    if (result.stdout || result.stderr || result.overflowed || result.value !== count) {
        throw new Error('Fresh job local child process failed');
    }
    return result.value;
}
