import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { runJavaScript } from 'wasm-rquickjs:execution';
import { ok, processExitStatusSchema, toolDefinition } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs';
import { z } from '../../../../sdks/ts/packages/golem-ts-sdk/node_modules/zod/v4/index.js';
export { golemTool010Guest, golemTool010Guest as guest } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs';
export { guest as golemAgent200Guest, toolMiddlewareGuest as golemTool010ToolMiddlewareGuest, saveSnapshot, loadSnapshot } from '../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/emptyGuest.mjs';

const probe = toolDefinition('node-tool-probe', { requiresFilesystem: true }).version('0.0.1')
  .body(body => body.option('cwd', z.string(), { default: '/workspace' })
    .tail('args', z.string(), { separator: '--', verbatim: true })
    .stdout({ required: true }).stderr({ required: true }).returns(processExitStatusSchema));

probe.implement({
  async 'node-tool-probe'(input, context) {
    if (globalThis.nodeToolProbeInvoked || process.env.GOLEM_NODE_TOOL_PROBE_INVOCATION) throw new Error('Previous tool invocation leaked state');
    globalThis.nodeToolProbeInvoked = true;
    process.env.GOLEM_NODE_TOOL_PROBE_INVOCATION = 'active';
    const [mode, content] = input.args;
    if (!['baseline', 'nested', 'job-baseline', 'job-nested', 'verify'].includes(mode)) throw new Error('Invalid mode');
    if (mode !== 'verify') mkdirSync(input.cwd, { recursive: true });
    const path = `${input.cwd}/node-tool-process-probe.txt`;
    if (mode.startsWith('job-')) {
      const execution = await runJavaScript({ source: `
        if ('nodeToolProbeInvoked' in globalThis) throw new Error('Parent global leaked into job');
        const fs = await import('node:fs');
        ${mode === 'job-nested' ? `
          const { spawnSync } = await import('node:child_process');
          const result = spawnSync('write-file', [${JSON.stringify(path)}, ${JSON.stringify(content)}]);
          if (result.error || result.status !== 0 || result.stderr.length) throw new Error('Fresh-job CLI tool invocation failed: ' + result.stderr + ': ' + result.error);
        ` : `fs.writeFileSync(${JSON.stringify(path)}, ${JSON.stringify(content)});`}
        return fs.readFileSync(${JSON.stringify(path)}, 'utf8');
      `, cwd: input.cwd, timeoutMs: 5000, maxBytes: 4096 });
      if (execution.value !== content || execution.stdout || execution.stderr || execution.overflowed) throw new Error('Unexpected job output');
    } else if (mode === 'nested') {
      const help = spawnSync('write-file', ['--help']);
      if (help.error || help.status !== 0 || !help.stdout.length || help.stderr.length) throw new Error('CLI help failed');
      process.env.GOLEM_NODE_TOOL_PROCESSES = 'false';
      const disabledPath = `${input.cwd}/must-not-create.txt`;
      const disabled = spawnSync('write-file', [disabledPath, 'unexpected']);
      delete process.env.GOLEM_NODE_TOOL_PROCESSES;
      if (disabled.status !== null || !disabled.error || existsSync(disabledPath)) throw new Error('Disabled backend dispatched work');
      const result = spawnSync('write-file', [path, content]);
      if (result.error || result.status !== 0 || result.stderr.length || !result.stdout.length) throw new Error('CLI tool invocation failed: ' + result.stderr + ': ' + result.error);
    } else if (mode === 'baseline') writeFileSync(path, content);
    const visible = readFileSync(path, 'utf8');
    if (visible !== content) throw new Error('Owner filesystem did not observe child effect');
    const writer = context.stdout.getWriter();
    try { await writer.write(new TextEncoder().encode(`${mode}:${visible}\n`)); } finally { writer.releaseLock(); }
    return ok({ exit_code: 0 });
  },
});
