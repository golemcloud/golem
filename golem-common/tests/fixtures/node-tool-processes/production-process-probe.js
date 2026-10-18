import process from 'node:process';
import { runJavaScript } from 'wasm-rquickjs:execution';
import { ok, processExitStatusSchema, toolDefinition } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs';
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

// This function is also executed unchanged in a fresh JavaScript job. All commands
// use canonical CLI argv and the shared production backend, not prepared wire inputs.
async function exerciseProcesses(mode) {
    const { spawnSync, spawn, execFile } = await import('node:child_process');
    const { Buffer } = await import('node:buffer');
    const { default: runtimeProcess } = await import('node:process');
    const check = (condition, message) => {
        if (!condition) throw new Error(message);
    };
    const options = { maxBuffer: 4096 };
    const dualArgs = ['dual-reconstruct', 'redaction-terminals'];
    const describe = result => {
        check(!result.error && result.status === 0 && result.signal === null,
            `Tool command failed: ${result.error?.message ?? result.status}: ${result.stderr}`);
        check(result.stdout.equals(Buffer.from('marker:')), 'Streamed stdout changed');
        check(result.stderr.equals(Buffer.from('stderr-visible')), 'Missing independent stderr');
        return [7, result.stdout.subarray(0, 7).reduce((sum, value) => sum + value, 0),
            result.stderr.length, result.stderr.reduce((sum, value) => sum + value, 0)].join(':');
    };
    const dual = describe(spawnSync('streaming', dualArgs, options));
    if (mode === 'streams-collect') return dual;
    const structured = spawnSync('streaming', ['no-stream', 'structured'], options);
    check(!structured.error && structured.status === 0 &&
        structured.stdout.toString('utf8') === 'no-stream:structured' && structured.stderr.length === 0,
        'A command without stdout did not render its structured result');

    const timeout = spawnSync('clock-race', ['race-p3-sleeps', '600'],
        { ...options, timeout: 100 });
    check(timeout.error?.code === 'ETIMEDOUT' && timeout.status === null && timeout.signal === 'SIGTERM',
        `Sync timeout projection failed: ${timeout.error?.message}: ${timeout.stderr}`);
    const followup = spawnSync('clock-race', ['follow-up'], { ...options, timeout: 5000 });
    check(!followup.error && followup.status === 0 && followup.stdout.toString('utf8').includes('settled'),
        `Follow-up after timeout failed: ${followup.error?.message}: ${followup.stderr}`);

    const collectChild = (command, args, input, childOptions = options, onSpawn) => new Promise((resolve, reject) => {
        const child = spawn(command, args, childOptions);
        const stdout = [];
        const stderr = [];
        const events = [];
        child.stdout?.on('data', bytes => stdout.push(Buffer.from(bytes)));
        child.stderr?.on('data', bytes => stderr.push(Buffer.from(bytes)));
        child.once('error', reject);
        child.once('spawn', () => {
            events.push('spawn');
            onSpawn?.(child);
            if (input !== undefined) child.stdin.end(input);
        });
        child.once('exit', () => events.push('exit'));
        child.once('close', (status, signal) => {
            events.push('close');
            resolve({ status, signal, stdout: Buffer.concat(stdout), stderr: Buffer.concat(stderr), events });
        });
    });
    const asyncDual = await collectChild('streaming', dualArgs);
    check(describe(asyncDual) === dual && asyncDual.events.join(',') === 'spawn,exit,close',
        'Async spawn output or event order differs');
    const callbackDual = await new Promise((resolve, reject) => {
        execFile('streaming', dualArgs, { ...options, encoding: 'buffer' }, (error, stdout, stderr) => {
            if (error) reject(error);
            else resolve({ status: 0, signal: null, stdout, stderr });
        });
    });
    check(describe(callbackDual) === dual, 'execFile callback output differs');

    const bytes = Buffer.from([0, 255, 1, 128, 0, 13, 10, 254]);
    const echo = await collectChild('streaming', ['run', 'echo'], bytes);
    check(echo.status === 0 && echo.signal === null && echo.stdout.equals(bytes) && echo.stderr.length === 0,
        'Async stdin did not preserve binary bytes');

    const ignored = spawnSync('streaming', ['produce', '2', '32'],
        { stdio: 'ignore', maxBuffer: 0 });
    check(!ignored.error && ignored.status === 0 && ignored.stdout === null && ignored.stderr === null,
        'Ignored output retained bytes or cancelled the tool');
    const inherited = [];
    const originalWrite = runtimeProcess.stdout.write;
    runtimeProcess.stdout.write = (chunk, callback) => {
        inherited.push(Buffer.from(chunk));
        callback?.();
        return true;
    };
    try {
        const syncInherited = spawnSync('streaming', dualArgs,
            { ...options, stdio: ['ignore', 'inherit', 'pipe'] });
        check(!syncInherited.error && syncInherited.status === 0 && syncInherited.stdout === null &&
            syncInherited.stderr.equals(Buffer.from('stderr-visible')) &&
            Buffer.concat(inherited).subarray(0, 7).equals(Buffer.from('marker:')),
            'Sync inherited stdout was not routed independently');
        inherited.length = 0;
        const asyncInherited = await collectChild('streaming', dualArgs, undefined,
            { ...options, stdio: ['ignore', 'inherit', 'pipe'] });
        check(asyncInherited.status === 0 && asyncInherited.stdout.length === 0 &&
            Buffer.concat(inherited).subarray(0, 7).equals(Buffer.from('marker:')),
            'Async inherited stdout was not joined');
    } finally {
        runtimeProcess.stdout.write = originalWrite;
    }
    const cancelled = await collectChild('clock-race', ['race-p3-sleeps', '600'], undefined,
        options, child => check(child.kill('SIGTERM'), 'Accepted tool refused cancellation'));
    check(cancelled.status === null && cancelled.signal === 'SIGTERM', 'Async cancellation projection failed');
    return 'cancelled:followup-ok';
}

probe.implement({
    async 'node-tool-probe'(input, context) {
        if (globalThis.nodeToolProbeInvoked || process.env.GOLEM_NODE_TOOL_PROBE_INVOCATION) {
            throw new Error('Previous invocation leaked globals or environment');
        }
        globalThis.nodeToolProbeInvoked = true;
        process.env.GOLEM_NODE_TOOL_PROBE_INVOCATION = 'active';
        const [mode, effectUrl] = input.args;
        if (!['streams-deadline-collect', 'streams-collect'].includes(mode)) throw new Error(`Unexpected mode: ${mode}`);
        if (effectUrl) {
            const response = await fetch(`${effectUrl}/${mode}-main`, { method: 'POST' });
            if (response.status !== 204) throw new Error('Unexpected main effect status');
            await response.arrayBuffer();
        }
        const main = await exerciseProcesses(mode);
        const job = await runJavaScript({ source: `
            if ('nodeToolProbeInvoked' in globalThis) throw new Error('Fresh job inherited parent globals');
            const effectUrl = ${JSON.stringify(effectUrl)};
            const mode = ${JSON.stringify(mode)};
            if (effectUrl) {
                const response = await fetch(effectUrl + '/' + mode + '-job', { method: 'POST' });
                if (response.status !== 204) throw new Error('Unexpected fresh-job effect status');
                await response.arrayBuffer();
            }
            return await (${exerciseProcesses.toString()})(mode);
        `, cwd: '/', timeoutMs: 10000, maxBytes: 65536 });
        if (job.value !== main || job.stdout || job.stderr || job.overflowed) {
            throw new Error('Production process results differ in a fresh runtime');
        }
        const writer = context.stdout.getWriter();
        try { await writer.write(new TextEncoder().encode(`${mode}:${main}\n`)); }
        finally { writer.releaseLock(); }
        return ok({ exit_code: 0 });
    },
});
