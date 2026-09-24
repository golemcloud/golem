import { command, err, ok, s, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

const Choice = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('left'), count: s.u32() }),
  z.object({ kind: z.literal('right'), names: z.array(z.string().nullable()) }),
]);

const Tree: z.ZodType<{ label: string; children: unknown[] }> = z.lazy(() =>
  z.object({ label: z.string(), children: z.array(Tree) }),
);

const Definition = toolDefinition('compiled')
  .command('nested', (nested) =>
    nested
      .aliases('n')
      .body((body) => body.positional('names', z.array(z.string().nullable())).returns(Choice))
      .command('fail', (fail) =>
        fail.body((body) =>
          body
            .positional('code', s.u32())
            .error('broken', {
              kind: 'runtime',
              exitCode: 13,
              payload: z.object({ code: s.u32() }),
            }),
        ),
      ),
  )
  .command('tree', (tree) => tree.body((body) => body.positional('tree', Tree).returns(Tree)))
  .command('secret', (secret) =>
    secret.body((body) =>
      body
        .positional('value', z.object({ secret: s.secret(z.string()), count: s.u32() }))
        .returns(z.object({ secret: s.secret(z.string()), count: s.u32() })),
    ),
  );

Definition.implement({
  nested: command(({ names }) => ok({ kind: 'right', names }), {
    fail: ({ code }) => err('broken', { code: code + 5 }),
  }),
  tree: ({ tree }) => ok(tree),
  secret: ({ value }) => ok(value),
});
