// Feasibility fixture only. The original sync modes use an experimental
// invoke-and-await lowering; collect modes use ordinary native bindings.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { ToolRpc, createOutput, createCollectedOutput, createStdin } from 'golem:tool/host@0.1.0';
import { runJavaScript } from 'wasm-rquickjs:execution';
import {
    ok,
    processExitStatusSchema,
    toolDefinition,
} from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs';
import { getToolType } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/toolReflection.mjs';
import { typedSchemaValueToWit } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/internal/schema-model/wit.mjs';
import { z } from '../../../../sdks/ts/packages/golem-ts-sdk/node_modules/zod/v4/index.js';

export {
    golemTool010Guest,
    golemTool010Guest as guest,
} from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs';
export {
    guest as golemAgent200Guest,
    toolMiddlewareGuest as golemTool010ToolMiddlewareGuest,
    saveSnapshot,
    loadSnapshot,
} from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/emptyGuest.mjs';

const probe = toolDefinition('node-tool-probe', { requiresFilesystem: true })
    .version('0.0.1')
    .body(body => body
        .option('cwd', z.string(), { default: '/workspace' })
        .tail('args', z.string(), { separator: '--', verbatim: true })
        .stdout({ required: true })
        .stderr({ required: true })
        .returns(processExitStatusSchema));

function callSynchronously(input) {
    const rpc = new ToolRpc('write-file');
    const result = rpc.invokeAndAwait([], input, undefined, undefined, undefined);
    if (result && typeof result.then === 'function') {
        throw new Error('Expected synchronous native ToolRpc binding');
    }
    if (!result?.result) throw new Error('write-file returned no structured result');
}

// Deliberately uses JS-owned pumps on both paths. This tests whether changing
// only the terminal import's lowering is sufficient; it is not the backend.
async function probeStreams(mode) {
    const echo = mode.includes('echo');
    const pressure = mode.includes('pressure');
    const cancel = mode === 'streams-cancel-async';
    const path = [echo || cancel ? 'run' : pressure ? 'produce' : 'dual-reconstruct'];
    const command = getToolType('streaming').command(path);
    const wireInput = typedSchemaValueToWit({
        graph: command.inputSchema.graph,
        value: command.packJson(echo || cancel ? { mode: cancel ? 'stdin-or-clock' : 'echo' } : pressure
            ? { 'chunk-count': 512, 'chunk-size': 65536 }
            : { mode: 'redaction-terminals' }),
    });
    let stdin;
    let inputWriter;
    if (echo || cancel) {
        const [writer, source, closed] = createStdin();
        // Preselect stdin before entering the blocking import: this isolates
        // output scheduling from a separate JS input-producer dependency.
        if (!cancel) {
            await writer.write(new Uint8Array([0, 255, 1, 128, 0, 13, 10, 254]));
            await writer.finish();
        }
        inputWriter = writer;
        stdin = source;
        closed[Symbol.dispose]?.();
    }
    const [stdoutTarget, stdout] = createOutput();
    const stderrEndpoints = !echo && !pressure && !cancel ? createOutput() : undefined;
    let future;
    async function drain(source) {
        let bytes = 0;
        let checksum = 0;
        for await (const item of source) {
            if (item.tag !== 'ok') {
                if (cancel && item.val.tag === 'cancelled') break;
                throw new Error(`Child output failure: ${JSON.stringify(item)}`);
            }
            bytes += item.val.length;
            for (const byte of item.val) checksum = (checksum + byte) >>> 0;
            // The child writes this marker before racing open stdin against a
            // ten-minute clock wait. Cancel only after observing actual work.
            if (cancel && bytes === 7) future.cancel();
        }
        return { bytes, checksum };
    }
    const outputs = Promise.all([
        drain(stdout),
        stderrEndpoints ? drain(stderrEndpoints[1]) : Promise.resolve({ bytes: 0, checksum: 0 }),
    ]);
    // Start the JS readers before the synchronous native import. This does not
    // assume that their native tasks can keep progressing while JS is paused.
    await Promise.resolve();
    const rpc = new ToolRpc('streaming');
    const args = [path, wireInput, stdin, stdoutTarget, stderrEndpoints?.[0]];
    future = mode.endsWith('-async') ? rpc.asyncInvokeAndAwait(...args) : undefined;
    let cancelled = false;
    try {
        const result = future ? await future.get() : rpc.invokeAndAwait(...args);
        if (!result?.result) throw new Error('Missing child structured result');
    } catch (error) {
        if (!cancel || error.tag !== 'cancelled') throw error;
        cancelled = true;
    } finally {
        inputWriter?.[Symbol.dispose]?.();
    }
    if (cancel && !cancelled) throw new Error('Child did not return the cancellation terminal');
    const [out, err] = await outputs;
    future?.[Symbol.dispose]?.();
    return `${mode}:${out.bytes}:${out.checksum}:${err.bytes}:${err.checksum}${cancel ? ':cancelled' : ''}\n`;
}

async function probeCollectedStreams(mode) {
    const echo = mode.includes('echo');
    const pressure = mode.includes('pressure');
    const path = [echo ? 'run' : pressure ? 'produce' : 'dual-reconstruct'];
    const command = getToolType('streaming').command(path);
    const wireInput = typedSchemaValueToWit({
        graph: command.inputSchema.graph,
        value: command.packJson(echo ? { mode: 'echo' } : pressure
            ? { 'chunk-count': 512, 'chunk-size': 65536 }
            : { mode: 'redaction-terminals' }),
    });
    let stdin;
    if (echo) {
        const [writer, source, closed] = createStdin();
        await writer.write(new Uint8Array([0, 255, 1, 128, 0, 13, 10, 254]));
        await writer.finish();
        writer[Symbol.dispose]?.();
        closed[Symbol.dispose]?.();
        stdin = source;
    }
    const [stdoutTarget, stdout] = createCollectedOutput();
    const stderr = !echo && !pressure ? createCollectedOutput() : undefined;
    const rpc = new ToolRpc('streaming');
    const future = rpc.asyncInvokeAndAwait(path, wireInput, stdin, stdoutTarget, stderr?.[0]);
    // A regular synchronous WIT import: no experimental lowering override,
    // Promise continuation or JavaScript output pump participates in this wait.
    const collected = future.collect(stdout, stderr?.[1], pressure ? 4096n : 65536n);
    if (collected && typeof collected.then === 'function') {
        throw new Error('Expected synchronous native output collection');
    }
    future[Symbol.dispose]?.();
    if (!collected.stdout) throw new Error('Missing collected stdout');
    if (pressure) {
        if (!collected.stdout.overflow || collected.stdout.bytes.length !== 4096) {
            throw new Error(`Unexpected bounded output: ${JSON.stringify(collected)}`);
        }
        if (collected.result.tag !== 'err' || collected.result.val.tag !== 'cancelled') {
            throw new Error(`Output overflow did not cancel the child: ${JSON.stringify(collected.result)}`);
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
            throw new Error('Previous tool invocation leaked globals or environment');
        }
        globalThis.nodeToolProbeInvoked = true;
        process.env.GOLEM_NODE_TOOL_PROBE_INVOCATION = 'active';
        const [mode, content = 'nested-owner-data'] = input.args;
        if (['streams-collect', 'streams-echo-collect', 'streams-pressure-collect'].includes(mode)) {
            const text = await probeCollectedStreams(mode);
            const writer = context.stdout.getWriter();
            try { await writer.write(new TextEncoder().encode(text)); }
            finally { writer.releaseLock(); }
            return ok({ exit_code: 0 });
        }
        if (/^streams-(echo-|pressure-)?(sync|async)$/.test(mode) || mode === 'streams-cancel-async') {
            const text = await probeStreams(mode);
            const writer = context.stdout.getWriter();
            try { await writer.write(new TextEncoder().encode(text)); }
            finally { writer.releaseLock(); }
            return ok({ exit_code: 0 });
        }
        if (!['baseline', 'nested', 'job-baseline', 'job-nested', 'verify'].includes(mode)) {
            throw new Error(`Unexpected probe mode: ${mode}`);
        }
        if (mode !== 'verify') mkdirSync(input.cwd, { recursive: true });
        const path = `${input.cwd}/node-tool-process-probe.txt`;
        let wireInput;
        if (mode.endsWith('nested')) {
            const command = getToolType('write-file').command([]);
            wireInput = typedSchemaValueToWit({
                graph: command.inputSchema.graph,
                value: command.packJson({ path, content, 'create-parent-directories': false }),
            });
        }
        if (mode.startsWith('job-')) {
            const execution = await runJavaScript({
                source: `
                    if ('nodeToolProbeInvoked' in globalThis) throw new Error('Fresh job inherited parent globals');
                    const fs = await import('node:fs');
                    ${mode === 'job-nested' ? `
                        const { ToolRpc } = await import('golem:tool/host@0.1.0');
                        const rpc = new ToolRpc('write-file');
                        const result = rpc.invokeAndAwait([], ${JSON.stringify(wireInput)}, undefined, undefined, undefined);
                        if (result && typeof result.then === 'function') throw new Error('Expected sync native binding in fresh job');
                        if (!result?.result) throw new Error('Missing child result');
                    ` : `fs.writeFileSync(${JSON.stringify(path)}, ${JSON.stringify(content)});`}
                    return fs.readFileSync(${JSON.stringify(path)}, 'utf8');
                `,
                cwd: input.cwd,
                timeoutMs: 5000,
                maxBytes: 4096,
            });
            if (execution.value !== content || execution.stdout || execution.stderr || execution.overflowed) {
                throw new Error(`Unexpected execution result: ${JSON.stringify(execution)}`);
            }
        } else if (mode === 'nested') {
            callSynchronously(wireInput);
        } else if (mode === 'baseline') {
            writeFileSync(path, content);
        }
        const visible = readFileSync(path, 'utf8');
        if (visible !== content) throw new Error(`Owner filesystem has stale data: ${visible}`);
        const writer = context.stdout.getWriter();
        try {
            await writer.write(new TextEncoder().encode(`${mode}:${visible}\n`));
        } finally {
            writer.releaseLock();
        }
        return ok({ exit_code: 0 });
    },
});
