// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { collectSync } from 'golem-tool-process-bridge';
import { runJavaScript } from 'wasm-rquickjs:execution';
import { ok, processExitStatusSchema, toolDefinition } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs';
import { getToolType } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/toolReflection.mjs';
import { typedSchemaValueToWit } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/internal/schema-model/wit.mjs';
import { z } from '../../../../sdks/ts/packages/golem-ts-sdk/node_modules/zod/v4/index.js';

export { golemTool010Guest, golemTool010Guest as guest } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs';
export { guest as golemAgent200Guest, toolMiddlewareGuest, toolMiddlewareGuest as golemTool010ToolMiddlewareGuest, saveSnapshot, loadSnapshot } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/emptyGuest.mjs';

const probe = toolDefinition('node-tool-probe', { requiresFilesystem: true })
    .version('0.0.1')
    .body(body => body.option('cwd', z.string(), { default: '/workspace' })
        .tail('args', z.string(), { separator: '--', verbatim: true })
        .stdout({ required: true }).stderr({ required: true })
        .returns(processExitStatusSchema));

probe.implement({
    async 'node-tool-probe'(input, context) {
        const [mode, effectUrl] = input.args;
        const echo = mode.includes('echo');
        const pressure = mode.includes('pressure');
        const path = [echo ? 'run' : pressure ? 'produce' : 'dual-reconstruct'];
        const command = getToolType('streaming').command(path);
        const wire = typedSchemaValueToWit({
            graph: command.inputSchema.graph,
            value: command.packJson(echo ? { mode: 'echo' } : pressure
                ? { 'chunk-count': 512, 'chunk-size': 65536 }
                : { mode: 'redaction-terminals' }),
        });
        if (effectUrl) {
            const response = await fetch(`${effectUrl}/${mode}-main`, { method: 'POST' });
            await response.arrayBuffer();
        }
        const bytes = echo ? new Uint8Array([0, 255, 1, 128, 0, 13, 10, 254]) : new Uint8Array();
        const value = collectSync('streaming', path, wire, bytes, echo, !echo && !pressure, 4096);
        if (value && typeof value.then === 'function') throw new Error('Expected synchronous guest bridge');
        if (!echo) {
            const job = await runJavaScript({ source: `
                const { collectSync } = await import('golem-tool-process-bridge');
                if (${JSON.stringify(effectUrl)} !== undefined) {
                    const response = await fetch(${JSON.stringify(effectUrl)} + '/' + ${JSON.stringify(mode)} + '-job', { method: 'POST' });
                    await response.arrayBuffer();
                }
                return collectSync('streaming', ${JSON.stringify(path)}, ${JSON.stringify(wire)}, new Uint8Array(), false, ${!pressure}, 4096);
            `, cwd: '/', timeoutMs: 10000, maxBytes: 4096 });
            if (job.value !== value || job.stdout || job.stderr || job.overflowed) throw new Error('Fresh runtime differs');
        }
        const writer = context.stdout.getWriter();
        try { await writer.write(new TextEncoder().encode(`${mode}:${value}\n`)); }
        finally { writer.releaseLock(); }
        return ok({ exit_code: 0 });
    },
});
