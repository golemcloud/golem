import { describe, expect, it } from 'vitest';
import { z } from 'zod';
import '../src/schema/zod';
import { registerAgentType } from '../src/runtime';
import { s } from '../src/schema/markers';
import { AgentStream } from '../src/schema/agentStream';
import { schemaValueFromWit, schemaValueToWit, v } from '../src/internal/schema-model';

describe('prepared runtime codecs', () => {
  it('keeps field order and principal injection separate from wire field count', () => {
    const agent = registerAgentType(
      'prepared-input',
      {},
      {
        read: {
          input: { before: z.string(), principal: s.principal(), after: s.u8() },
          returns: z.void(),
        },
      },
    );
    const runtime = agent.runtimeMethods.get('read')!;
    for (const n of [7, 31]) {
      const wire = schemaValueToWit(v.record([v.string('first'), v.u8(n)]));
      expect(runtime.read(wire, { tag: 'anonymous' })).toMatchObject({
        before: 'first',
        principal: { tag: 'anonymous' },
        after: n,
      });
    }
    expect(() =>
      runtime.read(schemaValueToWit(v.record([v.string('first')])), { tag: 'anonymous' }),
    ).toThrow(/2 user-supplied fields/);
  });

  it('retains generic output semantics rather than revalidating or reapplying source transforms', async () => {
    let transforms = 0;
    const agent = registerAgentType(
      'prepared-output',
      {},
      {
        echo: {
          input: {},
          returns: z.object({
            label: z
              .string()
              .regex(new RegExp('^item-'))
              .transform((value) => {
                transforms++;
                return `${value}!`;
              }),
            count: s.u8(),
          }),
        },
      },
    );
    const runtime = agent.runtimeMethods.get('echo')!;
    for (const n of [7, 255]) {
      const wire = await runtime.write({ label: 'item-ready!', count: n, extra: 'ignored' });
      expect(schemaValueFromWit(wire!)).toEqual(v.record([v.string('item-ready!'), v.u8(n)]));
    }
    expect(transforms).toBe(0);
    await expect(runtime.write({ label: 'item-ready!', count: 256 })).rejects.toThrow();
  });

  it('keeps asynchronous wrapping and affine ownership for nested stream outputs', async () => {
    const agent = registerAgentType(
      'prepared-stream',
      {},
      {
        stream: { input: {}, returns: z.object({ values: s.stream(s.u8()) }) },
      },
    );
    expect(agent.methodCodecs.get('stream')!.output).toMatchObject({ tag: 'single' });
    const output = agent.methodCodecs.get('stream')!.output;
    if (output.tag !== 'single') throw new Error('expected output');
    expect(output.codec.direct).toBeUndefined();
    let pulls = 0;
    const stream = AgentStream.from(
      (async function* () {
        pulls++;
        yield 23;
      })(),
    );
    const runtime = agent.runtimeMethods.get('stream')!;
    const wire = await runtime.write({ values: stream });
    expect(pulls).toBe(0);
    const decoded = output.codec.fromValue(schemaValueFromWit(wire!)) as {
      values: AgentStream<number>;
    };
    expect(await decoded.values.next()).toEqual({ done: false, value: 23 });
    expect(pulls).toBe(1);
    await expect(runtime.write({ values: stream })).rejects.toThrow();
    await decoded.values.return();
  });
});
