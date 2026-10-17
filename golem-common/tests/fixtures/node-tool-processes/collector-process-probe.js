import { spawnSync } from 'node:child_process';
import { ToolRpc, createCollectedOutput, createStdin } from 'golem:tool/host@0.1.0';
import { runJavaScript } from 'wasm-rquickjs:execution';
import { ok, processExitStatusSchema, toolDefinition } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs';
import { getToolType } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/toolReflection.mjs';
import { typedSchemaValueToWit } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/internal/schema-model/wit.mjs';
import { z } from '../../../../sdks/ts/packages/golem-ts-sdk/node_modules/zod/v4/index.js';

export { golemTool010Guest, golemTool010Guest as guest } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs';
export {
    guest as golemAgent200Guest,
    toolMiddlewareGuest as golemTool010ToolMiddlewareGuest,
    saveSnapshot, loadSnapshot,
} from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/emptyGuest.mjs';

const probe = toolDefinition('node-tool-probe', { requiresFilesystem: true })
    .version('0.0.1')
    .body(body => body
        .option('cwd', z.string(), { default: '/workspace' })
        .tail('args', z.string(), { separator: '--', verbatim: true })
        .stdout({ required: true }).stderr({ required: true })
        .returns(processExitStatusSchema));

function summarize(output) {
    const channel = value => value && {
        bytes: Array.from(value.bytes), overflow: value.overflow, failure: value.failure,
    };
    return JSON.stringify({
        stdout: channel(output.stdout), stderr: channel(output.stderr),
        terminal: output.result.tag === 'ok' ? 'ok' : output.result.val.tag,
    });
}

async function collect(mode, effectUrl) {
    const echo = mode === 'streams-echo-collect';
    const pressure = mode === 'streams-pressure-collect';
    const path = [echo ? 'run' : pressure ? 'produce' : 'dual-reconstruct'];
    const command = getToolType('streaming').command(path);
    const wireInput = typedSchemaValueToWit({
        graph: command.inputSchema.graph,
        value: command.packJson(echo ? { mode: 'echo' } : pressure
            ? { 'chunk-count': 512, 'chunk-size': 65536 }
            : { mode: 'redaction-terminals' }),
    });
    let collected;
    if (effectUrl) {
        const response = await fetch(`${effectUrl}/${mode}-main`, { method: 'POST' });
        if (response.status !== 204) throw new Error(`Unexpected effect status: ${response.status}`);
        await response.arrayBuffer();
    }
    if (echo) {
        // Control only: prewrite finite input before the blocking call.
        // This does not prove Node input pumping while QuickJS is paused.
        const [writer, stdin, closed] = createStdin();
        await writer.write(new Uint8Array([0, 255, 1, 128, 0, 13, 10, 254]));
        await writer.finish();
        writer[Symbol.dispose]?.();
        closed[Symbol.dispose]?.();
        const [target, stdout] = createCollectedOutput();
        const future = new ToolRpc('streaming').asyncInvokeAndAwait(path, wireInput, stdin, target, undefined);
        collected = future.collect(stdout, undefined, 65536n);
        future[Symbol.dispose]?.();
    } else {
        const request = JSON.stringify({ path, input: wireInput, stderr: !pressure });
        const result = spawnSync('streaming', [request], { maxBuffer: pressure ? 4096 : 65536 });
        collected = result.collected;
        if (!collected || result.stdout.length !== collected.stdout.bytes.length) {
            throw new Error('Process backend did not preserve collected output');
        }
        const job = await runJavaScript({
            source: `
                if ('nodeToolProbeInvoked' in globalThis) throw new Error('Fresh job inherited parent globals');
                const effectUrl = ${JSON.stringify(effectUrl)};
                if (effectUrl) {
                    const response = await fetch(effectUrl + ${JSON.stringify(`/${mode}-job`)}, { method: 'POST' });
                    if (response.status !== 204) throw new Error('Unexpected fresh job effect status');
                    await response.arrayBuffer();
                }
                const { spawnSync } = await import('node:child_process');
                const result = spawnSync('streaming', [${JSON.stringify(request)}], {
                    maxBuffer: ${pressure ? 4096 : 65536},
                });
                if (!result.collected) throw new Error('Fresh job has no process backend');
                return (${summarize.toString()})(result.collected);
            `,
            cwd: '/', timeoutMs: 10000, maxBytes: 65536,
        });
        if (job.value !== summarize(collected) || job.stdout || job.stderr || job.overflowed) {
            throw new Error(`Fresh job process output differs: ${JSON.stringify(job)}`);
        }
    }
    if (!collected?.stdout || typeof collected.then === 'function') {
        throw new Error('Missing synchronous collected stdout');
    }
    if (pressure) {
        if (!collected.stdout.overflow || collected.stdout.bytes.length !== 4096 ||
            collected.result.tag !== 'err' || collected.result.val.tag !== 'cancelled') {
            throw new Error(`Overflow did not bound output and cancel the child: ${JSON.stringify(collected)}`);
        }
        return `${mode}:overflow:4096\n`;
    }
    if (collected.result.tag !== 'ok' || !collected.result.val.result) {
        throw new Error(`Unexpected child result: ${JSON.stringify(collected.result)}`);
    }
    const describe = output => {
        if (!output) return [0, 0];
        if (output.failure || output.overflow) throw new Error('Unexpected collected output failure');
        return [output.bytes.length, output.bytes.reduce((sum, byte) => sum + byte, 0)];
    };
    return `${mode}:${[...describe(collected.stdout), ...describe(collected.stderr)].join(':')}\n`;
}

probe.implement({
    async 'node-tool-probe'(input, context) {
        if (globalThis.nodeToolProbeInvoked || process.env.GOLEM_NODE_TOOL_PROBE_INVOCATION) {
            throw new Error('Previous invocation leaked globals or environment');
        }
        globalThis.nodeToolProbeInvoked = true;
        process.env.GOLEM_NODE_TOOL_PROBE_INVOCATION = 'active';
        const [mode, effectUrl] = input.args;
        if (!['streams-collect', 'streams-echo-collect', 'streams-pressure-collect'].includes(mode)) {
            throw new Error(`Unexpected probe mode: ${mode}`);
        }
        const writer = context.stdout.getWriter();
        try { await writer.write(new TextEncoder().encode(await collect(mode, effectUrl))); }
        finally { writer.releaseLock(); }
        return ok({ exit_code: 0 });
    },
});
