import { execute } from 'experiment:node-tools/host';
import { runJavaScript } from 'wasm-rquickjs:execution';

export async function run(value) {
    const result = execute(value);
    if (result && typeof result.then === 'function') {
        throw new Error('Expected a synchronous native import, not a Promise');
    }
    return result;
}

export async function runJob(value) {
    globalThis.parentMarker = 'parent';
    const result = await runJavaScript({
        source: `
            if ('parentMarker' in globalThis) throw new Error('Inherited parent globals');
            const { execute } = await import('experiment:node-tools/host');
            const value = execute(${JSON.stringify(value)});
            if (value && typeof value.then === 'function') throw new Error('Expected sync import');
            return value;
        `,
        cwd: '/',
        timeoutMs: 3000,
        maxBytes: 1024,
    });
    if (result.stdout || result.stderr || result.overflowed) throw new Error('Unexpected job output');
    return result.value;
}
