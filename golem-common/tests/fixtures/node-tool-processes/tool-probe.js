// Feasibility fixture only: invoke-and-await is synchronously lowered by the
// generated native binding. This is not the proposed child_process API.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { ToolRpc } from 'golem:tool/host@0.1.0';
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

probe.implement({
    async 'node-tool-probe'(input, context) {
        if (globalThis.nodeToolProbeInvoked || process.env.GOLEM_NODE_TOOL_PROBE_INVOCATION) {
            throw new Error('Previous tool invocation leaked globals or environment');
        }
        globalThis.nodeToolProbeInvoked = true;
        process.env.GOLEM_NODE_TOOL_PROBE_INVOCATION = 'active';
        const [mode, content = 'nested-owner-data'] = input.args;
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
