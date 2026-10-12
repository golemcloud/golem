// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { ok, s, toolDefinition } from '../src/index';
import { ToolRegistry } from '../src/internal/registry/toolRegistry';
import { prepareNativeProcess } from '../src/internal/tool/nativeProcess';
import { t, v, typedSchemaValueFromWit, typedSchemaValueToWit } from '../src/internal/schema-model';

beforeEach(() => ToolRegistry.clearForTests());

function inputFor(name: string, path: string[], values: Record<string, unknown>) {
  const registered = ToolRegistry.get(name)!;
  const node = registered.extended.commandByPath(path)!;
  return typedSchemaValueToWit(registered.extended.canonicalInputModel(node).encodeTyped(values));
}

function registerProcess() {
  const handler = vi.fn(() => ok(0));
  toolDefinition('process')
    .body((body) =>
      body
        .option('working-directory', z.string(), { required: true })
        .stdout({ required: true })
        .stderr({ required: true })
        .returns(s.s32()),
    )
    .implement({ process: handler });
  return handler;
}

describe('native process preparation', () => {
  it('decodes input and prepares the result before execution without calling the handler', async () => {
    const handler = registerProcess();
    const prepared = await prepareNativeProcess(
      'process',
      [],
      inputFor('process', [], {
        'working-directory': '/workspace',
      }),
    );
    expect(prepared.input).toEqual({ workingDirectory: '/workspace' });
    expect(typedSchemaValueFromWit(prepared.resultTemplate).value).toEqual({
      tag: 's32',
      value: 0,
    });
    expect(prepared.stdoutRequired).toBe(true);
    expect(prepared.stderrRequired).toBe(true);
    expect(handler).not.toHaveBeenCalled();
  });

  it('does not change ordinary invocation', async () => {
    const handler = registerProcess();
    const wire = inputFor('process', [], { 'working-directory': '/workspace' });
    await prepareNativeProcess('process', [], wire);
    expect(handler).not.toHaveBeenCalled();
    await ToolRegistry.resolveInvocation('process', [])
      .prepareWire(wire)
      .invoke({ principal: 'test' });
    expect(handler).toHaveBeenCalledOnce();
    expect(handler).toHaveBeenCalledWith({ workingDirectory: '/workspace' }, { principal: 'test' });
  });

  it('preserves command forwarding without running the child handler', async () => {
    const handler = vi.fn(() => ok(0));
    const child = toolDefinition('run').body((body) =>
      body.option('cwd', z.string(), { required: true }).returns(s.s32()),
    );
    child.implement({ run: handler });
    toolDefinition('parent').command('run', child).implement({});
    const prepared = await prepareNativeProcess(
      'parent',
      ['run'],
      inputFor('parent', ['run'], { cwd: '/' }),
    );
    expect(prepared.input).toEqual({ cwd: '/' });
    expect(prepared.stdoutRequired).toBe(false);
    expect(prepared.stderrRequired).toBe(false);
    expect(handler).not.toHaveBeenCalled();
  });

  it('preserves command aliases and collection inputs', async () => {
    const handler = vi.fn(() => ok(0));
    toolDefinition('cli')
      .command('run', (run) =>
        run
          .aliases('r')
          .body((body) =>
            body.option('args', z.array(z.string()), { required: true }).returns(s.s32()),
          ),
      )
      .implement({ run: handler });
    const wire = inputFor('cli', ['run'], { args: ['--noEmit', 'index.ts'] });
    const prepared = await prepareNativeProcess('cli', ['r'], wire);
    expect(prepared.input).toEqual({ args: ['--noEmit', 'index.ts'] });
    expect(handler).not.toHaveBeenCalled();
  });

  it('rejects unknown names and paths', async () => {
    registerProcess();
    const wire = inputFor('process', [], { 'working-directory': '/' });
    await expect(prepareNativeProcess('missing', [], wire)).rejects.toEqual({
      tag: 'invalid-tool-name',
      val: 'missing',
    });
    await expect(prepareNativeProcess('process', ['missing'], wire)).rejects.toEqual({
      tag: 'invalid-command-path',
      val: ['missing'],
    });
  });

  it('rejects malformed input without running the handler', async () => {
    const handler = registerProcess();
    const wire = inputFor('process', [], { 'working-directory': '/' });
    wire.value.valueNodes = [];
    await expect(prepareNativeProcess('process', [], wire)).rejects.toMatchObject({
      tag: 'invalid-input',
    });
    expect(handler).not.toHaveBeenCalled();
  });

  it('checks the supplied graph even when its value has the expected shape', async () => {
    const handler = registerProcess();
    const wire = inputFor('process', [], { 'working-directory': '/' });
    const wrong = typedSchemaValueToWit({
      graph: { defs: new Map(), root: t.string() },
      value: v.string('/'),
    });
    wire.graph = wrong.graph;
    await expect(prepareNativeProcess('process', [], wire)).rejects.toMatchObject({
      tag: 'invalid-input',
    });
    expect(handler).not.toHaveBeenCalled();
  });

  it('rejects malformed unused nodes before the direct decoder can ignore them', async () => {
    const handler = registerProcess();
    const wire = inputFor('process', [], { 'working-directory': '/' });
    wire.value.valueNodes.push({ tag: 'list-value', val: [999999] });
    await expect(prepareNativeProcess('process', [], wire)).rejects.toMatchObject({
      tag: 'invalid-input',
    });
    expect(handler).not.toHaveBeenCalled();
  });

  it('rejects a result that the native exit-code encoder cannot represent', async () => {
    const handler = vi.fn(() => ok('done'));
    toolDefinition('text')
      .body((body) => body.returns(z.string()))
      .implement({ text: handler });
    await expect(prepareNativeProcess('text', [], inputFor('text', [], {}))).rejects.toMatchObject({
      tag: 'invalid-result',
      val: 'native process commands require an unrestricted s32 result',
    });
    expect(handler).not.toHaveBeenCalled();
  });

  it('rejects restricted exit codes rather than returning a template with weaker semantics', async () => {
    const handler = vi.fn(() => ok(0));
    toolDefinition('restricted')
      .body((body) => body.returns(s.s32({ min: 0 })))
      .implement({ restricted: handler });
    await expect(
      prepareNativeProcess('restricted', [], inputFor('restricted', [], {})),
    ).rejects.toMatchObject({ tag: 'invalid-result' });
    expect(handler).not.toHaveBeenCalled();
  });

  it('rejects unsupported input metadata before decoding or adopting values', async () => {
    const handler = vi.fn(() => ok(0));
    toolDefinition('wide')
      .body((body) => body.option('value', s.u64(), { required: true }).returns(s.s32()))
      .implement({ wide: handler });
    const wire = inputFor('wide', [], { value: 0n });
    await expect(prepareNativeProcess('wide', [], wire)).rejects.toMatchObject({
      tag: 'invalid-result',
      val: 'native process commands require plain scalar and collection inputs',
    });
    expect(handler).not.toHaveBeenCalled();
  });

  it('rejects stdin and failed registration before execution', async () => {
    const handler = vi.fn(() => ok(0));
    toolDefinition('input')
      .body((body) => body.stdin({ required: true }).returns(s.s32()))
      .implement({ input: handler });
    const wire = inputFor('input', [], {});
    await expect(prepareNativeProcess('input', [], wire)).rejects.toMatchObject({
      tag: 'invalid-result',
      val: 'native process commands cannot declare stdin',
    });
    ToolRegistry.recordRegistrationError('input', 'broken registration');
    await expect(prepareNativeProcess('input', [], wire)).rejects.toMatchObject({
      tag: 'invalid-result',
      val: 'tool registration failed',
    });
    expect(handler).not.toHaveBeenCalled();
  });
});
