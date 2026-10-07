// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { describe, expect, it, vi } from 'vitest';
import { AgentStream, client, s, toolDefinition, type ToolClientTransport } from '../src';
import { compileSchema } from '../src/schema/adapter';
import {
  schemaGraphToWit,
  schemaValueToWitAsync,
  typedSchemaValueFromWit,
} from '../src/internal/schema-model';
import { createToolClientRuntime } from '../src/bridge/tool';

const streamSchema = s.stream(s.u32());
const streamCodec = compileSchema(streamSchema);
const definition = toolDefinition('gol40-client-stream-acceptance').body((body) =>
  body.positional('input', streamSchema).returns(streamSchema),
);

describe('GOL-40 TypeScript CLIENT-STREAM acceptance', () => {
  it('asynchronously encodes the native stream passed by a generated client runtime', async () => {
    const cancel = vi.fn();
    const start = vi.fn<ToolClientTransport['start']>((_path, input) => {
      const decoded = typedSchemaValueFromWit(input);
      const source = streamCodec.fromValue(decoded.value) as AgentStream<number>;
      return {
        settledResult: (async () => {
          const values: number[] = [];
          for await (const value of source) values.push(value);
          expect(values).toEqual([2, 5, 9]);
          return { status: 'fulfilled' as const, value: {} };
        })(),
        cancel,
      };
    });
    const runtime = createToolClientRuntime('generated-runtime', { start });

    const invocation = runtime.start(
      ['typed', 'transform'],
      { graph: streamCodec.graph, value: streamCodec.toValue(AgentStream.from([2, 5, 9])) },
      undefined,
      false,
      false,
    );

    await expect(invocation.settledResult).resolves.toEqual({
      status: 'fulfilled',
      value: { result: undefined },
    });
    expect(start).toHaveBeenCalledOnce();
    invocation.cancel();
    expect(cancel).toHaveBeenCalledOnce();
  });

  it('transforms generated-client typed streams with the exact asymmetric tuple', async () => {
    const start = vi.fn<ToolClientTransport['start']>((_path, input) => ({
      settledResult: (async () => {
        const decoded = typedSchemaValueFromWit(input);
        if (decoded.value.tag !== 'record') throw new Error('expected canonical input record');
        const source = streamCodec.fromValue(decoded.value.fields[0]) as AgentStream<number>;
        const received: number[] = [];
        for await (const value of source) received.push(value);
        expect(received).toEqual([2, 5, 9]);

        const transformed = AgentStream.from(received.map((value) => value * 3 + 1));
        return {
          status: 'fulfilled' as const,
          value: {
            result: {
              graph: schemaGraphToWit(streamCodec.graph),
              value: await schemaValueToWitAsync(streamCodec.toValue(transformed)),
            },
          },
        };
      })(),
      cancel() {},
    }));
    const generated = client(definition, { transport: { start } });

    const output = await generated['gol40-client-stream-acceptance']({
      input: AgentStream.from([2, 5, 9]),
    });
    const values: number[] = [];
    for await (const value of output) values.push(value);

    expect(values).toEqual([7, 16, 28]);
    expect(start).toHaveBeenCalledOnce();
  });
});
