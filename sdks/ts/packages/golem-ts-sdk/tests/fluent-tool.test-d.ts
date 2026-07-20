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

// Type-only coverage for the complete fluent git command tree. Checked via
// `tsc --noEmit`; NOT executed by vitest (`.test-d.ts` suffix).

import { z } from 'zod/v4';
import {
  KeyValue,
  Path,
  c,
  command,
  err,
  ok,
  s,
  toolDefinition,
  type ToolImplementation,
} from '../src/fluent';

const CommitResult = z.object({
  hash: z.string(),
  filesChanged: z.number().int(),
  insertions: z.number().int(),
  deletions: z.number().int(),
});

const gitDef = toolDefinition('git')
  .doc({ summary: 'Stupid content tracker' })
  .command('commit', (commit) =>
    commit
      .aliases('ci')
      .annotations({ destructive: true, idempotent: false, openWorld: false })
      .body((body) =>
        body
          .option('message', z.string(), { short: 'm', aliases: ['msg'], required: true })
          .option('author', z.string(), { env: 'GIT_AUTHOR_NAME' })
          .option('output', z.enum(['human', 'porcelain', 'json']), { default: 'human' })
          .flag('amend', { negatable: true, default: false })
          .flag('signoff', { negatable: true, default: false })
          .flag('reset-author')
          .constraint(c.implies({ lhs: c.present('reset-author'), rhs: c.present('amend') }))
          .constraint(c.requiresAll([c.valueIs('output', 'json')]))
          .returns(CommitResult, {
            formatters: ['human', 'porcelain', 'json'],
            defaultFormatter: 'human',
          })
          .error('nothing-staged', { kind: 'runtime', exitCode: 1 })
          .error('bad-author-format', {
            kind: 'usage',
            exitCode: 129,
            payload: z.object({ author: z.string() }),
          }),
      ),
  )
  .command('remote', (remote) =>
    remote
      .aliases('rmt')
      .command('add', (add) =>
        add.body((body) =>
          body
            .positional('name', z.string(), { valueName: 'NAME' })
            .positional('url', s.url(), { valueName: 'URL' })
            .option('track', z.string(), {
              short: 't',
              repeatable: 'repeated',
              valueName: 'BRANCH',
            })
            .option('master', z.string(), { short: 'm', valueName: 'BRANCH' })
            .flag('tags', { negatable: true, default: true })
            .flag('fetch', { short: 'f' })
            .returns(z.void())
            .error('no-such-remote', {
              kind: 'usage',
              exitCode: 128,
              payload: z.object({ name: z.string() }),
            }),
        ),
      )
      .command('remove', (remove) =>
        remove
          .aliases('rm')
          .body((body) =>
            body
              .positional('name', z.string())
              .returns(z.void())
              .error('no-such-remote', { kind: 'usage', exitCode: 128 }),
          ),
      )
      .command('set-url', (setUrl) =>
        setUrl.body((body) =>
          body
            .positional('name', z.string())
            .positional('newurl', s.url())
            .positional('oldurl', s.url(), { required: false })
            .flag('add')
            .flag('delete')
            .constraint(c.mutexGroups([[c.present('add')], [c.present('delete')]]))
            .returns(z.void()),
        ),
      ),
  )
  .command('log', (log) =>
    log.annotations({ readOnly: true, idempotent: true }).body((body) =>
      body
        .tail('paths', Path({ direction: 'input', kind: 'any' }), {
          separator: '--',
          min: 0,
        })
        .option('max-count', s.u32({ min: 0 }), { short: 'n' })
        .option('since', s.datetime())
        .option('author', z.string(), { repeatable: 'delimited', delim: ',' })
        .option('grep', z.string(), { repeatable: 'either', delim: ',' })
        .flag('all-match')
        .flag('invert-grep')
        .flag('oneline')
        .flag('graph')
        .constraint(c.allOrNone([c.present('all-match'), c.present('grep')]))
        .constraint(c.requiresAny([c.present('author'), c.present('grep')]))
        .constraint(c.forbids({ lhs: c.present('oneline'), rhs: c.present('graph') }))
        .returns(
          z.array(
            z.object({
              hash: z.string(),
              author: z.string(),
              date: z.string(),
              message: z.string(),
            }),
          ),
          {
            formatters: ['oneline', 'short', 'medium', 'full'],
            defaultFormatter: 'medium',
          },
        )
        .error('bad-revision', { kind: 'usage', exitCode: 128 }),
    ),
  )
  // Globals deliberately follow the command declarations. Their inferred fields
  // must still reach every descendant leaf.
  .global('git-dir', Path({ direction: 'in-out', kind: 'directory' }), {
    default: '.git',
    env: 'GIT_DIR',
  })
  .global('config', KeyValue(z.string()), {
    short: 'c',
    repeatable: 'repeated',
  })
  .global('verbose', { kind: 'count-flag', short: 'v', max: 3 })
  .global('paginate', z.boolean(), {
    kind: 'flag',
    negatable: true,
    default: true,
  });

const gitImplementation: ToolImplementation<typeof gitDef> = {
  commit: async (args) => {
    const message: string = args.message;
    const author: string | undefined = args.author;
    const output: 'human' | 'porcelain' | 'json' = args.output;
    const gitDir: string = args.gitDir;
    const config: Map<string, string> = args.config;
    const verbose: number = args.verbose;
    const paginate: boolean = args.paginate;
    const amend: boolean = args.amend;
    // @ts-expect-error metadata names are projected to camelCase argument keys
    void args['git-dir'];
    void author;
    void output;
    void gitDir;
    void config;
    void verbose;
    void paginate;
    void amend;
    if (message.length === 0) return err('nothing-staged');
    if (author === '') return err('bad-author-format', { author });
    return ok({ hash: 'abc', filesChanged: 0, insertions: 0, deletions: 0 });
  },
  remote: command({
    add: async (args) => {
      const track: string[] = args.track;
      const master: string | undefined = args.master;
      const gitDir: string = args.gitDir;
      const tags: boolean = args.tags;
      void track;
      void master;
      void gitDir;
      void tags;
      if (args.name === '') return err('no-such-remote', { name: args.name });
      return ok(undefined);
    },
    remove: async (args) => {
      const name: string = args.name;
      void name;
      return ok(undefined);
    },
    'set-url': async (args) => {
      const oldurl: string | undefined = args.oldurl;
      const add: boolean = args.add;
      void oldurl;
      void add;
      return ok(undefined);
    },
  }),
  log: async (args) => {
    const paths: string[] = args.paths;
    const maxCount: number | undefined = args.maxCount;
    const authors: string[] = args.author;
    const greps: string[] = args.grep;
    const allMatch: boolean = args.allMatch;
    void paths;
    void maxCount;
    void authors;
    void greps;
    void allMatch;
    return ok([]);
  },
};
void gitDef.implement(gitImplementation);

const grepDef = toolDefinition('grep')
  .body((body) =>
    body
      .positional('pattern', z.string())
      .stdin({ mime: ['text/plain'], required: false })
      .stdout({ mime: ['text/plain'], required: true })
      .returns(z.array(z.string()))
      .error('invalid-pattern', {
        kind: 'usage',
        exitCode: 2,
        payload: z.object({ reason: z.string() }),
      }),
  )
  .command('replace', (replace) =>
    replace
      .body((body) => body.returns(z.void()))
      .command('dry-run', (dryRun) => dryRun.body((body) => body.returns(z.boolean()))),
  );

const grepImplementation: ToolImplementation<typeof grepDef> = {
  grep: async (args, context) => {
    const pattern: string = args.pattern;
    const stdin: ReadableStream<Uint8Array> | undefined = context.stdin;
    const stdout: WritableStream<Uint8Array> = context.stdout;
    void pattern;
    void stdin;
    void stdout;
    return ok([]);
  },
  replace: command(async () => ok(undefined), {
    'dry-run': async () => ok(true),
  }),
};
void grepDef.implement(grepImplementation);

const payloadlessErrorDef = toolDefinition('payloadless-error').body((body) =>
  body.returns(z.void()).error('plain-error', { kind: 'runtime', exitCode: 1 }),
);

const payloadlessErrorImplementation: ToolImplementation<typeof payloadlessErrorDef> = {
  // @ts-expect-error plain-error does not declare a payload
  'payloadless-error': async () => {
    return err('plain-error', { unexpected: true });
  },
};
void payloadlessErrorImplementation;

const payloadlessUndefinedErrorImplementation: ToolImplementation<typeof payloadlessErrorDef> = {
  // @ts-expect-error plain-error does not declare a payload, including an explicit undefined payload
  'payloadless-error': async () => err('plain-error', undefined),
};
void payloadlessUndefinedErrorImplementation;

const projectedResultDef = toolDefinition('projected-result').body((body) =>
  body
    .returns(z.string())
    .error('plain-error', { kind: 'runtime', exitCode: 1 })
    .error('detailed-error', {
      kind: 'usage',
      exitCode: 2,
      payload: z.object({ reason: z.string() }),
    }),
);

const wrongSuccessImplementation: ToolImplementation<typeof projectedResultDef> = {
  // @ts-expect-error projected-result must return ok(string)
  'projected-result': async () => ok(42),
};
void wrongSuccessImplementation;

const undeclaredErrorImplementation: ToolImplementation<typeof projectedResultDef> = {
  // @ts-expect-error handlers can only return errors declared by their body
  'projected-result': async () => err('undeclared-error'),
};
void undeclaredErrorImplementation;

const missingErrorPayloadImplementation: ToolImplementation<typeof projectedResultDef> = {
  // @ts-expect-error detailed-error requires its declared payload
  'projected-result': async () => err('detailed-error'),
};
void missingErrorPayloadImplementation;

const wrongErrorPayloadImplementation: ToolImplementation<typeof projectedResultDef> = {
  // @ts-expect-error detailed-error payload must match the schema output type
  'projected-result': async () => err('detailed-error', { reason: 42 }),
};
void wrongErrorPayloadImplementation;

const unitResultDef = toolDefinition('unit-result').body((body) => body.returns(z.void()));
const wrongUnitResultImplementation: ToolImplementation<typeof unitResultDef> = {
  // @ts-expect-error unit results must be returned as ok(undefined)
  'unit-result': async () => ok('unexpected'),
};
void wrongUnitResultImplementation;

const optionalStreamDef = toolDefinition('optional-stream').body((body) =>
  body.stdin({ required: false }).returns(z.void()),
);
type OptionalStreamContext = Parameters<
  ToolImplementation<typeof optionalStreamDef>['optional-stream']
>[1];
const contextWithoutOptionalStdin: OptionalStreamContext = {
  principal: undefined as never,
};
void contextWithoutOptionalStdin;

const invalidCountFlagDef = toolDefinition('invalid-count-flag').body((body) =>
  body.flag('verbose', {
    kind: 'count-flag',
    // @ts-expect-error count flags do not have boolean defaults
    default: true,
    // @ts-expect-error count flags do not support negation
    negatable: true,
  }),
);
void invalidCountFlagDef;

const recordMapDef = toolDefinition('record-map').body((body) =>
  body.option('labels', z.record(z.string(), z.string()), { repeatable: 'repeated' }),
);
const recordMapImplementation: ToolImplementation<typeof recordMapDef> = {
  'record-map': async (args) => {
    const labels: Record<string, string> = args.labels;
    // @ts-expect-error repeatable WIT maps are collected as one map, not an array of maps
    const labelList: Record<string, string>[] = args.labels;
    void labels;
    void labelList;
    return ok(undefined);
  },
};
void recordMapImplementation;

const replacedTailDef = toolDefinition('replaced-tail').body((body) =>
  body.tail('first', z.string()).tail('second', z.number()),
);
const replacedTailImplementation: ToolImplementation<typeof replacedTailDef> = {
  'replaced-tail': async (args) => {
    // @ts-expect-error replacing the tail removes the old tail from the canonical input
    void args.first;
    const second: number[] = args.second;
    void second;
    return ok(undefined);
  },
};
void replacedTailImplementation;

const defaultedOptionalScalarDef = toolDefinition('defaulted-optional-scalar').body((body) =>
  body.option('mode', z.enum(['short', 'full']), {
    optionalScalar: true,
    default: 'short',
  }),
);
const defaultedOptionalScalarImplementation: ToolImplementation<typeof defaultedOptionalScalarDef> =
  {
    'defaulted-optional-scalar': async (args) => {
      const mode: 'short' | 'full' = args.mode;
      void mode;
      return ok(undefined);
    },
  };
void defaultedOptionalScalarImplementation;

const subtreeRemoteDef = toolDefinition('subtree-remote').body((body) =>
  body.positional('name', z.string()).returns(z.string()),
);
const subtreeParentDef = toolDefinition('subtree-parent').command(
  'subtree-remote',
  subtreeRemoteDef,
);
const subtreeParentImplementation: ToolImplementation<typeof subtreeParentDef> = {};
void subtreeParentImplementation;
const invalidSubtreeParentImplementation: ToolImplementation<typeof subtreeParentDef> = {
  // @ts-expect-error a separately grafted child is implemented by its own definition
  'subtree-remote': async () => ok('wrong owner'),
};
void invalidSubtreeParentImplementation;

// @ts-expect-error the root implicit-body key is the tool metadata name, not camelCase
const wrongRootKey: ToolImplementation<typeof grepDef> = { grepTool: async () => ok([]) };
void wrongRootKey;
