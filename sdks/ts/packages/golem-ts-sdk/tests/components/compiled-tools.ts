import { command, err, ok, s, toolDefinition } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';
import * as v from 'valibot';

const Choice = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('left'), count: s.u32() }),
  z.object({ kind: z.literal('right'), names: z.array(z.string().nullable()) }),
]);

const Tree: z.ZodType<{ label: string; children: unknown[] }> = z.lazy(() =>
  z.object({ label: z.string(), children: z.array(Tree) }),
);
const ValibotChoice = v.variant('kind', [
  v.object({ kind: v.literal('count'), value: s.u32() }),
  v.object({ kind: v.literal('label'), value: v.string() }),
]);

const Definition = toolDefinition('compiled')
  .command('nested', (nested) =>
    nested
      .aliases('n')
      .body((body) => body.positional('names', z.array(z.string().nullable())).returns(Choice))
      .command('fail', (fail) =>
        fail.body((body) =>
          body.positional('code', s.u32()).error('broken', {
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
  )
  .command('concrete', (concrete) =>
    concrete.body((body) =>
      body.positional('bytes', s.uint16Array()).returns(s.unstructuredBinary()),
    ),
  )
  .command('restricted-binary', (restricted) =>
    restricted.body((body) =>
      body.returns(s.unstructuredBinary({ mimeTypes: ['application/allowed'] })),
    ),
  )
  .command('valibot', (valibot) => valibot.body((body) => body.returns(ValibotChoice)));

Definition.implement({
  nested: command(({ names }) => ok({ kind: 'right', names }), {
    fail: ({ code }) => err('broken', { code: code + 5 }),
  }),
  tree: ({ tree }) => ok(tree),
  secret: ({ value }) => ok(value),
  concrete: ({ bytes }) =>
    ok({
      tag: 'inline',
      val: new Uint8Array([bytes[0]!, bytes.length]),
      mimeType: 'application/test',
    }),
  'restricted-binary': () =>
    ok({ tag: 'inline', val: new Uint8Array([1]), mimeType: 'application/disallowed' }),
  valibot: () => ok({ kind: 'label', value: 'four' }),
});
