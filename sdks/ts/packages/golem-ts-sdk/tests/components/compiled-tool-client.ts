import { client, s, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

const remote = toolDefinition('remote')
  .command('asymmetric', (command) =>
    command.body((body) =>
      body
        .positional('input', z.object({ count: s.u32(), labels: z.array(z.string().nullable()) }))
        .returns(z.object({ label: z.string(), values: z.array(s.u16()) })),
    ),
  )
  .command('fail', (command) =>
    command.body((body) =>
      body.error('broken', { kind: 'runtime', exitCode: 3, payload: z.object({ code: s.u32() }) }),
    ),
  );

let asymmetricCalls = 0;
const remoteClient = client(remote, {
  transport: {
    start(path, input) {
      if (path[0] === 'fail') {
        return {
          settledResult: Promise.resolve({
            status: 'rejected',
            reason: {
              tag: 'remote-tool-error',
              val: {
                tag: 'custom-error',
                val: {
                  name: 'broken',
                  payload: {
                    graph: input.graph,
                    value: {
                      valueNodes: [
                        { tag: 'u32-value', val: 41 },
                        { tag: 'record-value', val: [0] },
                      ],
                      root: 1,
                    },
                  },
                },
              },
            },
          }),
          cancel() {},
        };
      }
      const validInput = input.value.valueNodes[0]?.tag === 'u32-value';
      asymmetricCalls++;
      return {
        settledResult: Promise.resolve({
          status: 'fulfilled',
          value: {
            result: {
              graph: input.graph,
              value:
                asymmetricCalls === 2
                  ? { valueNodes: [{ tag: 'bool-value', val: true }], root: 0 }
                  : {
                      valueNodes: [
                        { tag: 'string-value', val: validInput ? 'left' : 'bad-input' },
                        { tag: 'u16-value', val: 2 },
                        { tag: 'u16-value', val: 9 },
                        { tag: 'list-value', val: [1, 2] },
                        { tag: 'record-value', val: [0, 3] },
                      ],
                      root: 4,
                    },
            },
          },
        }),
        cancel() {},
      };
    },
  },
});

(
  globalThis as typeof globalThis & { __golemCompiledToolClient?: unknown }
).__golemCompiledToolClient = remoteClient;
