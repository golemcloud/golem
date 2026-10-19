// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { client, s, toolDefinition, type ToolClientTransport } from '../src';
import { createToolClientRuntime } from '../src/bridge/tool';
import { secretHandleToSchemaValue } from '../src/bridge/schema';
import { encodeTool } from '../src/internal/tool';
import { typedSchemaValueToWit, v } from '../src/internal/schema-model';
import { compileSchema } from '../src/schema/adapter';
import { getExtendedToolDefinition } from '../src/tool';
import { ToolType } from '../src/toolReflection';

const Evidence = z.object({
  provider: z.string(),
  principal: z.string(),
  capabilityName: z.string(),
});
const Denial = z.object({
  reason: z.string(),
  retryable: z.boolean(),
});

const definition = toolDefinition('gol40-reflection-acceptance').body((body) =>
  body
    .positional('secret', s.secret(z.string()))
    .returns(Evidence)
    .error('denied', { kind: 'runtime', exitCode: 77, payload: Denial }),
);

const expectedEvidence = {
  provider: 'typescript',
  principal: 'oidc:gol40-reflection',
  capabilityName: 'matrix-secret',
};
const expectedDenial = {
  name: 'denied',
  payload: { reason: 'asymmetric-denial', retryable: false },
};

function wire(schema: Parameters<typeof compileSchema>[0], value: unknown) {
  const codec = compileSchema(schema);
  return typedSchemaValueToWit({ graph: codec.graph, value: codec.toValue(value) });
}

function rejection(promise: Promise<unknown>): Promise<unknown> {
  return promise.then(
    () => ({ resolved: true }),
    (error) => error,
  );
}

describe('GOL-40 TypeScript SDK reflection acceptance', () => {
  it('keeps supported generated and reflected capability observations equivalent', async () => {
    let rejectNext = false;
    const starts = vi.fn<ToolClientTransport['start']>(() => {
      if (rejectNext) {
        rejectNext = false;
        return {
          settledResult: Promise.resolve({
            status: 'rejected',
            reason: {
              tag: 'remote-tool-error',
              val: {
                tag: 'custom-error',
                val: { name: 'denied', payload: wire(Denial, expectedDenial.payload) },
              },
            },
          }),
          cancel() {},
        };
      }
      return {
        settledResult: Promise.resolve({
          status: 'fulfilled',
          value: { result: wire(Evidence, expectedEvidence) },
        }),
        cancel() {},
      };
    });
    const transport: ToolClientTransport = { start: starts };
    const generated = client(definition, { transport });
    const reflected = new ToolType(
      {
        lookupName: 'gol40-reflection-acceptance',
        definition: encodeTool(getExtendedToolDefinition(definition)),
        implementedBy: { uuid: { highBits: 40n, lowBits: 2n } },
      },
      createToolClientRuntime('gol40-reflection-acceptance', transport),
    ).client.command([]);

    const generatedEvidence = await generated['gol40-reflection-acceptance']({
      secret: { id: 'generated-success' } as never,
    });
    const reflectedValue = await reflected.invokeValue(
      v.record([secretHandleToSchemaValue({ id: 'reflected-success' } as never)]),
    );
    if (reflectedValue === undefined || reflected.result === undefined) {
      throw new Error('reflected command omitted its declared result');
    }
    const reflectedEvidence = reflected.result.unpackJson(reflectedValue);

    rejectNext = true;
    const generatedFailure = await rejection(
      generated['gol40-reflection-acceptance']({
        secret: { id: 'generated-failure' } as never,
      }),
    );
    rejectNext = true;
    const reflectedFailure = await rejection(
      reflected.invokeValue(
        v.record([secretHandleToSchemaValue({ id: 'reflected-failure' } as never)]),
      ),
    );

    expect({ generatedEvidence, reflectedEvidence }).toEqual({
      generatedEvidence: expectedEvidence,
      reflectedEvidence: expectedEvidence,
    });
    expect(generatedFailure).toMatchObject({
      cause: {
        tag: 'tool',
        error: { name: expectedDenial.name, payload: expectedDenial.payload },
      },
    });
    expect(reflectedFailure).toMatchObject({
      cause: {
        tag: 'tool',
        error: { name: expectedDenial.name, value: expectedDenial.payload },
      },
    });
    expect(starts).toHaveBeenCalledTimes(4);
  });
});
