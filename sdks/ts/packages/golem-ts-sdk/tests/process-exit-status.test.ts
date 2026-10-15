import { describe, expect, it } from 'vitest';
import {
  compileSchema,
  processExitStatusSchema,
  toolDefinition,
  type ProcessExitStatus,
} from '../src/index';
import { getExtendedToolDefinition } from '../src/tool';
import {
  schemaGraphFromWit,
  schemaGraphToWit,
  schemaValueFromWit,
  schemaValueToWit,
} from '../src/schema/public';

describe('public process exit status contract', () => {
  it('retains the nominal status as a tool return schema', () => {
    const definition = toolDefinition('status-test').body((body) =>
      body.returns(processExitStatusSchema),
    );
    const codec = getExtendedToolDefinition(definition).commandByPath([])?.body?.result?.codec;
    expect(codec?.graph.root.body).toEqual({ tag: 'ref', id: 'golem.tool.ProcessExitStatus' });
    expect(codec?.toValue({ exit_code: 7 })).toEqual({
      tag: 'record',
      fields: [{ tag: 'u8', value: 7 }],
    });
  });

  it('preserves nominal identity and the exact record through the wire schema', () => {
    const codec = compileSchema(processExitStatusSchema);
    const graph = schemaGraphFromWit(schemaGraphToWit(codec.graph));
    expect(graph).toEqual(codec.graph);
    expect(graph.root.body).toEqual({ tag: 'ref', id: 'golem.tool.ProcessExitStatus' });
    expect([...graph.defs.keys()]).toEqual(['golem.tool.ProcessExitStatus']);
    expect(graph.defs.get('golem.tool.ProcessExitStatus')?.body.body).toMatchObject({
      tag: 'record',
      fields: [{ name: 'exit_code', body: { body: { tag: 'u8' } } }],
    });
  });

  it.each([0, 1, 127, 255])('encodes status %i as a record containing u8', (code) => {
    const codec = compileSchema(processExitStatusSchema);
    const status: ProcessExitStatus = { exit_code: code };
    const value = codec.toValue(status);
    expect(value).toEqual({ tag: 'record', fields: [{ tag: 'u8', value: code }] });
    expect(codec.fromValue(schemaValueFromWit(schemaValueToWit(value)))).toEqual(status);
  });

  it.each([-1, 256, 0.5, NaN, Infinity, -Infinity, '0', undefined])(
    'rejects invalid exit_code %s',
    async (code) => {
      expect(
        await processExitStatusSchema['~standard'].validate({ exit_code: code }),
      ).toHaveProperty('issues');
    },
  );
});
