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

// Type-only coverage for the complete canonical grep and git command trees. Checked by
// the package typecheck script; NOT executed by vitest (`.test-d.ts` suffix).

import type {
  InvocationResult as WireInvocationResult,
  Tool as WireTool,
  TypedSchemaValue as WireTypedSchemaValue,
} from 'golem:tool/common@0.1.0';
import * as middlewareEntry from '@golemcloud/golem-ts-sdk/middleware';
import { z } from 'zod/v4';
import {
  KeyValue,
  Path,
  c,
  command,
  err,
  ok,
  ToolInvokeError,
  s,
  toolDefinition,
  universalToolMiddleware,
  type ImplementedToolMiddleware,
  type Principal,
  type ToolClient,
  type ToolClientErrors,
  type ToolErr,
  type ToolImplementation,
  type ToolMiddlewareImplementation,
  type ToolMiddlewareOptions,
  type ToolUnderlying,
  type ToolUnderlyingErrors,
} from '../dist/index.mjs';

type Equal<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends <Value>() => Value extends Right ? 1 : 2
    ? true
    : false;
type Expect<Value extends true> = Value;

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
      .annotations({ destructive: true })
      .body((body) =>
        body
          .option('message', z.string(), { short: 'm', aliases: ['msg'], required: true })
          .option('author', z.string().regex(/^.+ <.+@.+>$/), { env: 'GIT_AUTHOR_NAME' })
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
          .error('dirty-merge', { kind: 'runtime', exitCode: 128 })
          .error('bad-author-format', {
            kind: 'usage',
            exitCode: 129,
            payload: z.object({ author: z.string() }),
          }),
      )
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
      }),
  )
  .command('remote', (remote) =>
    remote
      .aliases('rmt')
      .command('add', (add) =>
        add.body((body) =>
          body
            .positional('name', z.string().regex(/^[a-zA-Z][a-zA-Z0-9_-]*$/), {
              valueName: 'NAME',
            })
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
        remove.aliases('rm').body((body) =>
          body
            .positional('name', z.string().regex(/^[a-zA-Z][a-zA-Z0-9_-]*$/))
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
            .flag('push')
            .flag('add')
            .flag('delete')
            .constraint(c.mutexGroups([[c.present('add')], [c.present('delete')]]))
            .returns(z.void())
            .error('failed', {
              kind: 'runtime',
              exitCode: 1,
              payload: z.string(),
            }),
        ),
      )
      // These globals deliberately follow the child declarations. Their inferred
      // fields must still reach every descendant leaf in this subtree.
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
      }),
  )
  .command('stash', (stash) =>
    stash
      .body((body) =>
        body
          .option('message', z.string(), { short: 'm', required: true })
          .flag('keep-index', { short: 'k' })
          .returns(z.void())
          .error('no-such-stash', {
            kind: 'usage',
            exitCode: 128,
            payload: z.object({ name: z.string() }),
          }),
      )
      .command('pop', (pop) =>
        pop.body((body) =>
          body
            .positional('name', z.string(), { required: false })
            .option('index', s.u32(), { short: 'i' })
            .returns(z.void())
            .error('no-such-stash', {
              kind: 'usage',
              exitCode: 128,
              payload: z.object({ name: z.string() }),
            }),
        ),
      )
      .command('apply', (apply) =>
        apply.body((body) =>
          body
            .positional('name', z.string(), { required: false })
            .option('index', s.u32(), { short: 'i' })
            .returns(z.void())
            .error('no-such-stash', {
              kind: 'usage',
              exitCode: 128,
              payload: z.object({ name: z.string() }),
            }),
        ),
      )
      .global('git-dir', Path({ direction: 'in-out', kind: 'directory' }), {
        default: '.git',
        env: 'GIT_DIR',
      })
      .global('verbose', { kind: 'count-flag', short: 'v', max: 3 }),
  )
  .command('log', (log) =>
    log.annotations({ readOnly: true, idempotent: true }).body((body) =>
      body
        .tail('paths', Path({ direction: 'input', kind: 'any' }), {
          separator: '--',
          min: 0,
        })
        .option('max-count', s.s64({ min: 0, max: 9223372036854775807n }), { short: 'n' })
        .option('since', s.datetime())
        .option('until', s.datetime())
        .option('author', z.string(), { repeatable: 'delimited', delim: ',' })
        .option('grep', z.string(), { repeatable: 'either', delim: ',' })
        .flag('all-match')
        .flag('invert-grep')
        .flag('oneline')
        .flag('graph')
        .constraint(c.allOrNone([c.present('all-match'), c.present('grep')]))
        .returns(
          z.array(
            z.object({
              hash: z.string(),
              author: z.string(),
              date: s.datetime(),
              message: z.string(),
            }),
          ),
          {
            formatters: ['oneline', 'short', 'medium', 'full'],
            defaultFormatter: 'medium',
          },
        )
        .error('bad-revision', { kind: 'usage', exitCode: 128 })
        .error('not-a-repository', { kind: 'usage', exitCode: 129 }),
    ),
  );

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
      const config: Map<string, string> = args.config;
      const verbose: number = args.verbose;
      const paginate: boolean = args.paginate;
      const tags: boolean = args.tags;
      void track;
      void master;
      void gitDir;
      void config;
      void verbose;
      void paginate;
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
      const push: boolean = args.push;
      const add: boolean = args.add;
      void oldurl;
      void push;
      void add;
      return ok(undefined);
    },
  }),
  stash: command(
    async (args) => {
      const message: string = args.message;
      const keepIndex: boolean = args.keepIndex;
      const gitDir: string = args.gitDir;
      const verbose: number = args.verbose;
      // @ts-expect-error canonical stash does not inherit config
      void args.config;
      // @ts-expect-error canonical stash does not inherit paginate
      void args.paginate;
      void message;
      void keepIndex;
      void gitDir;
      void verbose;
      return ok(undefined);
    },
    {
      pop: async (args) => {
        const name: string | undefined = args.name;
        const index: number | undefined = args.index;
        const gitDir: string = args.gitDir;
        const verbose: number = args.verbose;
        void name;
        void index;
        void gitDir;
        void verbose;
        return ok(undefined);
      },
      apply: async (args) => {
        const name: string | undefined = args.name;
        const index: number | undefined = args.index;
        void name;
        void index;
        return ok(undefined);
      },
    },
  ),
  log: async (args) => {
    const paths: string[] = args.paths;
    const maxCount: bigint | undefined = args.maxCount;
    const until: { seconds: bigint; nanoseconds: number } | undefined = args.until;
    const authors: string[] = args.author;
    const greps: string[] = args.grep;
    const allMatch: boolean = args.allMatch;
    // @ts-expect-error canonical log does not inherit git-dir
    void args.gitDir;
    void paths;
    void maxCount;
    void until;
    void authors;
    void greps;
    void allMatch;
    return ok([
      {
        hash: 'abc',
        author: 'A. U. Thor',
        date: { seconds: 0n, nanoseconds: 0 },
        message: 'Initial commit',
      },
    ]);
  },
};
void gitDef.implement(gitImplementation);

const GrepHit = z.object({
  file: Path(),
  line: z.number().int(),
  text: z.string(),
});

const grepDef = toolDefinition('grep')
  .version('2.0.0')
  .global('case-sensitive', z.boolean(), { kind: 'flag', short: 'i' })
  .global('color', z.enum(['always', 'never', 'auto']), { default: 'auto' })
  .body((body) =>
    body
      .positional('pattern', z.string().regex(/^.+$/))
      .tail('files', Path({ direction: 'input' }), { acceptsStdio: true })
      .option('extra-patterns', z.string(), {
        short: 'e',
        repeatable: 'either',
        delim: ',',
      })
      .option('max-count', s.u32({ min: 1 }), { short: 'n' })
      .stdin({ mime: ['text/plain'], required: false })
      .stdout({ mime: ['text/plain'], required: true })
      .returns(z.array(GrepHit), {
        formatters: ['human', 'json'],
        defaultFormatter: 'human',
      })
      .error('invalid-pattern', {
        kind: 'usage',
        exitCode: 2,
        payload: z.object({ reason: z.string() }),
      })
      .error('no-match', { kind: 'runtime', exitCode: 1 }),
  )
  .command('replace', (replace) =>
    replace.body((body) =>
      body
        .positional('pattern', z.string().regex(/^.+$/))
        .positional('replacement', z.string())
        .tail('files', Path({ direction: 'in-out' }))
        .returns(s.u64())
        .error('invalid-pattern', {
          kind: 'usage',
          exitCode: 2,
          payload: z.object({ reason: z.string() }),
        })
        .error('no-match', { kind: 'runtime', exitCode: 1 }),
    ),
  );

const grepImplementation: ToolImplementation<typeof grepDef> = {
  grep: async (args, context) => {
    const pattern: string = args.pattern;
    const files: string[] = args.files;
    const extraPatterns: string[] = args.extraPatterns;
    const maxCount: number | undefined = args.maxCount;
    const caseSensitive: boolean = args.caseSensitive;
    const color: 'always' | 'never' | 'auto' = args.color;
    const stdin: ReadableStream<Uint8Array> | undefined = context.stdin;
    const stdout: WritableStream<Uint8Array> = context.stdout;
    void pattern;
    void files;
    void extraPatterns;
    void maxCount;
    void caseSensitive;
    void color;
    void stdin;
    void stdout;
    return ok([{ file: 'README.md', line: 1, text: 'Golem' }]);
  },
  replace: async (args) => {
    const replacement: string = args.replacement;
    const files: string[] = args.files;
    const caseSensitive: boolean = args.caseSensitive;
    void replacement;
    void files;
    void caseSensitive;
    return ok(0n);
  },
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

const dispatcherDef = toolDefinition('dispatcher').command('group', (group) =>
  group.command('leaf', (leaf) => leaf.body((body) => body.returns(z.void()))),
);
const invalidDispatcherImplementation: ToolImplementation<typeof dispatcherDef> = {
  // @ts-expect-error a pure dispatcher requires a nested command implementation
  group: async () => ok(undefined),
};
void invalidDispatcherImplementation;

// @ts-expect-error the root implicit-body key is the tool metadata name, not camelCase
const wrongRootKey: ToolImplementation<typeof grepDef> = { grepTool: async () => ok([]) };
void wrongRootKey;

declare const grepClient: ToolClient<typeof grepDef>;
declare const clientStdin: ReadableStream<Uint8Array>;
declare const legacyClientStdin: AsyncIterable<number>;
const grepCall = grepClient.grep({
  pattern: 'TODO',
  files: ['./src'],
  extraPatterns: [],
  caseSensitive: true,
  color: 'auto',
  stdin: clientStdin,
});
const grepResult: import('../src/bridge/tool').StartedToolInvocation<
  Array<{ file: unknown; line: number; text: string }>
> = grepCall;
type GrepClientResult = Expect<
  Equal<
    typeof grepCall,
    import('../src/bridge/tool').StartedToolInvocation<
      Array<{ file: unknown; line: number; text: string }>
    >
  >
>;
void grepResult;
void (undefined as unknown as GrepClientResult);

type GrepClientErrors = ToolClientErrors<(typeof grepClient)['grep']>;
const invalidPatternClientError: GrepClientErrors = err('invalid-pattern', { reason: 'bad' });
const noMatchClientError: GrepClientErrors = err('no-match');
// @ts-expect-error client methods retain only their declared custom errors
const undeclaredClientError: GrepClientErrors = err('other-error');
void invalidPatternClientError;
void noMatchClientError;
void undeclaredClientError;

const replaceResult: Promise<bigint> = grepClient.replace({
  pattern: 'TODO',
  replacement: 'DONE',
  files: ['./src'],
  caseSensitive: false,
  color: 'never',
});
type ReplaceClientResult = Expect<
  Equal<ReturnType<(typeof grepClient)['replace']>, Promise<bigint>>
>;
void replaceResult;
void (undefined as unknown as ReplaceClientResult);
// @ts-expect-error client argument fields use camelCase, not canonical metadata names
grepClient.replace({ pattern: 'x', replacement: 'y', files: [], 'case-sensitive': true });
grepClient.replace({
  pattern: 'x',
  replacement: 'y',
  files: [],
  caseSensitive: true,
  color: 'auto',
  // @ts-expect-error replace does not declare stdin
  stdin: new ReadableStream<Uint8Array>(),
});

declare const gitClient: ToolClient<typeof gitDef>;
type CommitClientArgs = Parameters<(typeof gitClient)['commit']>[0];
declare const commitClientArgs: CommitClientArgs;
const commitMessage: string = commitClientArgs.message;
const commitGitDir: string = commitClientArgs.gitDir;
const commitConfig: Map<string, string> = commitClientArgs.config;
const commitVerbose: number = commitClientArgs.verbose;
void commitMessage;
void commitGitDir;
void commitConfig;
void commitVerbose;
// @ts-expect-error command aliases are metadata-only and do not create client members
void gitClient.ci;
// @ts-expect-error pure dispatchers are nested objects, not callable methods
gitClient.remote({});
const remoteRemoveResult: Promise<void> = gitClient.remote.remove({
  name: 'origin',
  gitDir: '.git',
  config: new Map(),
  verbose: 0,
  paginate: false,
});
type UnitClientResult = Expect<
  Equal<ReturnType<(typeof gitClient.remote)['remove']>, Promise<void>>
>;
void remoteRemoveResult;
void (undefined as unknown as UnitClientResult);

const stashResult: Promise<void> = gitClient.stash({
  message: 'work',
  keepIndex: false,
  gitDir: '.git',
  verbose: 0,
});
const stashPopResult: Promise<void> = gitClient.stash.pop({
  gitDir: '.git',
  verbose: 0,
});
void stashResult;
void stashPopResult;

const requiredStdoutDef = toolDefinition('required-stdout').body((body) =>
  body.stdout({ required: true }).returns(z.void()),
);
declare const requiredStdoutClient: ToolClient<typeof requiredStdoutDef>;
const requiredStdout: import('../src/bridge/tool').StartedToolInvocation<undefined> =
  requiredStdoutClient['required-stdout']({});
type RequiredStdoutResult = Expect<
  Equal<
    ReturnType<(typeof requiredStdoutClient)['required-stdout']>,
    import('../src/bridge/tool').StartedToolInvocation<undefined>
  >
>;
void requiredStdout;
void (undefined as unknown as RequiredStdoutResult);
// @ts-expect-error root bodies use the exact metadata name, not a camelCase duplicate
requiredStdoutClient.requiredStdout({});

const optionalStdoutDef = toolDefinition('optional-stdout').body((body) =>
  body.stdout({ required: false }).returns(z.void()),
);
declare const optionalStdoutClient: ToolClient<typeof optionalStdoutDef>;
const optionalStdout: import('../src/bridge/tool').StartedToolInvocation<undefined> =
  optionalStdoutClient['optional-stdout']({});
type OptionalStdoutResult = Expect<
  Equal<
    ReturnType<(typeof optionalStdoutClient)['optional-stdout']>,
    import('../src/bridge/tool').StartedToolInvocation<undefined>
  >
>;
void optionalStdout;
void (undefined as unknown as OptionalStdoutResult);

const optionalStructuredStdoutDef = toolDefinition('optional-structured-stdout').body((body) =>
  body.stdout({ required: false }).returns(z.string()),
);
declare const optionalStructuredStdoutClient: ToolClient<typeof optionalStructuredStdoutDef>;
const optionalStructuredStdout: import('../src/bridge/tool').StartedToolInvocation<string> =
  optionalStructuredStdoutClient['optional-structured-stdout']({});
type OptionalStructuredStdoutResult = Expect<
  Equal<
    ReturnType<(typeof optionalStructuredStdoutClient)['optional-structured-stdout']>,
    import('../src/bridge/tool').StartedToolInvocation<string>
  >
>;
void optionalStructuredStdout;
void (undefined as unknown as OptionalStructuredStdoutResult);

declare const optionalStreamClient: ToolClient<typeof optionalStreamDef>;
const optionalStdinOmitted: Promise<void> = optionalStreamClient['optional-stream']({});
const optionalStdinSupplied: Promise<void> = optionalStreamClient['optional-stream']({
  stdin: clientStdin,
});
void optionalStdinOmitted;
void optionalStdinSupplied;

const requiredStdinDef = toolDefinition('required-stdin').body((body) =>
  body.stdin({ required: true }).returns(z.void()),
);
declare const requiredStdinClient: ToolClient<typeof requiredStdinDef>;
requiredStdinClient['required-stdin']({ stdin: clientStdin });
// @ts-expect-error required stdin must be supplied by the caller
requiredStdinClient['required-stdin']({});
requiredStdinClient['required-stdin']({
  // @ts-expect-error caller-side stdin must support cancellation of a pending read
  stdin: legacyClientStdin,
});

const clientSubtreeDef = toolDefinition('client-subtree')
  .global('child-global', z.string(), { required: true })
  .body((body) => body.positional('value', z.number()).returns(z.boolean()))
  .command('nested', (nested) => nested.body((body) => body.returns(z.string())));
const clientParentDef = toolDefinition('client-parent')
  .global('parent-global', z.boolean(), { kind: 'flag' })
  .command('client-subtree', clientSubtreeDef);
declare const clientParent: ToolClient<typeof clientParentDef>;
const subtreeResult: Promise<boolean> = clientParent['client-subtree']({
  parentGlobal: true,
  childGlobal: 'child',
  value: 1,
});
const nestedSubtreeResult: Promise<string> = clientParent['client-subtree'].nested({
  parentGlobal: true,
  childGlobal: 'child',
});
void subtreeResult;
void nestedSubtreeResult;
declare const standaloneSubtreeClient: ToolClient<typeof clientSubtreeDef>;
standaloneSubtreeClient['client-subtree']({ childGlobal: 'child', value: 1 });
standaloneSubtreeClient.nested({ childGlobal: 'child' });
standaloneSubtreeClient['client-subtree']({
  // @ts-expect-error the standalone subtree client does not inherit its graft parent's globals
  parentGlobal: true,
  childGlobal: 'child',
  value: 1,
});

const deprojectedSubtreeDef = toolDefinition('deprojected-subtree')
  .global('config', z.string(), { aliases: ['settings'], required: true })
  .body((body) =>
    body.option('region', z.string(), { aliases: ['location'], required: true }).returns(z.void()),
  )
  .command('nested', (nested) => nested.body((body) => body.returns(z.void())));
const deprojectedParentDef = toolDefinition('deprojected-parent')
  .global('profile', z.string(), { aliases: ['config'], optionalScalar: true })
  .global('deployment-region', z.string(), { aliases: ['region'], optionalScalar: true })
  .command('deprojected-subtree', deprojectedSubtreeDef);
declare const deprojectedParentClient: ToolClient<typeof deprojectedParentDef>;
deprojectedParentClient['deprojected-subtree']({});
deprojectedParentClient['deprojected-subtree']({
  profile: 'prod',
  deploymentRegion: 'eu-west',
});
deprojectedParentClient['deprojected-subtree'].nested({});
deprojectedParentClient['deprojected-subtree'].nested({
  profile: 'prod',
  deploymentRegion: 'eu-west',
});
deprojectedParentClient['deprojected-subtree']({
  // @ts-expect-error a grafted client exposes the ancestor canonical field, not the captured child field
  config: 'prod',
});
deprojectedParentClient['deprojected-subtree']({
  // @ts-expect-error graft-root body parameters captured by an ancestor are also omitted
  region: 'eu-west',
});
deprojectedParentClient['deprojected-subtree'].nested({
  // @ts-expect-error nested graft paths preserve the ancestor canonical field
  config: 'prod',
});

declare const standaloneDeprojectedClient: ToolClient<typeof deprojectedSubtreeDef>;
standaloneDeprojectedClient['deprojected-subtree']({ config: 'prod', region: 'eu-west' });
standaloneDeprojectedClient.nested({ config: 'prod' });
// @ts-expect-error the standalone child still requires its own global
standaloneDeprojectedClient['deprojected-subtree']({});
// @ts-expect-error the standalone nested path still requires the child global
standaloneDeprojectedClient.nested({});

const nonTransitiveLeafDef = toolDefinition('non-transitive-leaf')
  .global('config', z.string(), { required: true })
  .body((body) => body.returns(z.void()));
const nonTransitiveMiddleDef = toolDefinition('non-transitive-middle')
  .global('profile', z.string(), { aliases: ['config'], required: true })
  .command('non-transitive-leaf', nonTransitiveLeafDef);
const nonTransitiveRootDef = toolDefinition('non-transitive-root')
  .global('tenant', z.string(), { aliases: ['profile'], required: true })
  .command('non-transitive-middle', nonTransitiveMiddleDef);
declare const nonTransitiveClient: ToolClient<typeof nonTransitiveRootDef>;
nonTransitiveClient['non-transitive-middle']['non-transitive-leaf']({
  tenant: 'root',
  config: 'leaf',
});
// @ts-expect-error a removed middle declaration does not propagate its child-only alias
nonTransitiveClient['non-transitive-middle']['non-transitive-leaf']({ tenant: 'root' });

const grepMiddlewareImplementation: ToolMiddlewareImplementation<typeof grepDef> = {
  grep: async (args, context) => {
    const principal: Principal = context.principal;
    const stdin: AsyncIterable<number> | undefined = context.stdin;
    void principal;
    // @ts-expect-error middleware stdout is returned by the caller-side projection
    void context.stdout;
    return context.underlying.grep({ ...args, stdin });
  },
  replace: async (args, { underlying }) => underlying.replace(args),
};
const grepMiddleware: ImplementedToolMiddleware<'grep-middleware'> = grepDef.middleware({
  name: 'grep-middleware',
  aliases: ['grep-policy'],
  doc: 'Transparent grep policy',
  implementation: grepMiddlewareImplementation,
});
void grepMiddleware;

const inferredGrepMiddleware = grepDef.middleware({
  name: 'inferred-grep-middleware',
  implementation: {
    grep: async (args, context) => {
      const pattern: string = args.pattern;
      const stdin: AsyncIterable<number> | undefined = context.stdin;
      void pattern;
      // @ts-expect-error transparent middleware arguments follow the presented definition
      void args.unknownArgument;
      // @ts-expect-error middleware stdin is caller-side, not a web ReadableStream
      const webStdin: ReadableStream<Uint8Array> | undefined = context.stdin;
      // @ts-expect-error middleware has no writable stdout context
      void context.stdout;
      // @ts-expect-error typed underlying calls enforce presented command arguments
      void context.underlying.replace({ pattern: args.pattern });
      return context.underlying.grep({ ...args, stdin });
    },
    replace: async (args, { underlying }) => underlying.replace(args),
  },
});
const inferredGrepMiddlewareName: 'inferred-grep-middleware' = inferredGrepMiddleware.name;
void inferredGrepMiddlewareName;

const secondGrepMiddleware: ImplementedToolMiddleware<'second-grep-middleware'> =
  grepDef.middleware({
    name: 'second-grep-middleware',
    implementation: grepMiddlewareImplementation,
  });
void secondGrepMiddleware;

const invalidMiddlewareResult: ToolMiddlewareImplementation<typeof projectedResultDef> = {
  // @ts-expect-error middleware handlers return caller-side results, not leaf ok(...) wrappers
  'projected-result': async () => ok('unexpected-wrapper'),
};
void invalidMiddlewareResult;

const requiredStdinMiddleware: ToolMiddlewareImplementation<typeof requiredStdinDef> = {
  'required-stdin': async (args, context) => {
    const stdin: AsyncIterable<number> = context.stdin;
    return context.underlying['required-stdin']({ ...args, stdin });
  },
};
void requiredStdinMiddleware;

type OptionalMiddlewareContext = Parameters<
  ToolMiddlewareImplementation<typeof optionalStreamDef>['optional-stream']
>[1];
const optionalMiddlewareContextWithoutStdin: OptionalMiddlewareContext = {
  principal: undefined as never,
  underlying: undefined as never,
};
void optionalMiddlewareContextWithoutStdin;

const graftedMiddlewareImplementation: ToolMiddlewareImplementation<typeof subtreeParentDef> = {
  'subtree-remote': async (args, { underlying }) => underlying['subtree-remote'](args),
};
void graftedMiddlewareImplementation;
// @ts-expect-error middleware owns the complete presented tree, including grafted commands
const missingGraftedMiddlewareImplementation: ToolMiddlewareImplementation<
  typeof subtreeParentDef
> = {};
void missingGraftedMiddlewareImplementation;

const nestedGraftedMiddlewareImplementation: ToolMiddlewareImplementation<typeof clientParentDef> =
  {
    'client-subtree': command(async (args, { underlying }) => underlying['client-subtree'](args), {
      nested: async (args, { underlying }) => underlying['client-subtree'].nested(args),
    }),
  };
void nestedGraftedMiddlewareImplementation;

const expectedAdapterDef = toolDefinition('expected-adapter')
  .body((body) =>
    body
      .positional('path', z.string())
      .returns(z.number())
      .error('backend-failed', {
        kind: 'runtime',
        exitCode: 1,
        payload: z.object({ code: z.number() }),
      }),
  )
  .command('status', (status) => status.body((body) => body.returns(z.boolean())));
const presentedAdapterDef = toolDefinition('presented-adapter')
  .body((body) =>
    body
      .positional('path', z.string())
      .returns(z.string())
      .error('denied', { kind: 'usage', exitCode: 2 }),
  )
  .command('status', (status) => status.body((body) => body.returns(z.string())));

const adapterMiddlewareImplementation: ToolMiddlewareImplementation<
  typeof presentedAdapterDef,
  typeof expectedAdapterDef
> = {
  'presented-adapter': async (args, { underlying }) => {
    const result: number = await underlying['expected-adapter']({ path: args.path });
    return String(result);
  },
  status: async (_args, { underlying }) => String(await underlying.status({})),
};
const adapterMiddleware = presentedAdapterDef.middleware({
  name: 'adapter-middleware',
  wraps: expectedAdapterDef,
  implementation: adapterMiddlewareImplementation,
});
const adapterMiddlewareName: 'adapter-middleware' = adapterMiddleware.name;
void adapterMiddlewareName;

const inferredAdapterMiddleware = presentedAdapterDef.middleware({
  name: 'inferred-adapter-middleware',
  wraps: expectedAdapterDef,
  implementation: {
    'presented-adapter': async (args, { underlying }) => {
      const presentedPath: string = args.path;
      const expectedResult: number = await underlying['expected-adapter']({
        path: presentedPath,
      });
      // @ts-expect-error adapter underlying exposes the expected definition, not the presented one
      void underlying['presented-adapter'];
      // @ts-expect-error adapter underlying arguments follow the expected definition
      void underlying['expected-adapter']({ path: 42 });
      return String(expectedResult);
    },
    status: async (_args, { underlying }) => String(await underlying.status({})),
  },
});
const inferredAdapterMiddlewareName: 'inferred-adapter-middleware' = inferredAdapterMiddleware.name;
void inferredAdapterMiddlewareName;

// @ts-expect-error an expected-definition implementation requires the matching wraps definition
const adapterOptionsWithoutWraps: ToolMiddlewareOptions<
  'adapter-without-wraps',
  typeof presentedAdapterDef,
  typeof expectedAdapterDef
> = {
  name: 'adapter-without-wraps',
  implementation: adapterMiddlewareImplementation,
};
void adapterOptionsWithoutWraps;

declare const expectedUnderlying: ToolUnderlying<typeof expectedAdapterDef>;
type ExpectedUnderlyingErrors = ToolUnderlyingErrors<
  (typeof expectedUnderlying)['expected-adapter']
>;
const expectedUnderlyingError: ExpectedUnderlyingErrors = err('backend-failed', { code: 1 });
// @ts-expect-error typed underlying methods retain only expected-definition errors
const invalidExpectedUnderlyingError: ExpectedUnderlyingErrors = err('denied');
void expectedUnderlyingError;
void invalidExpectedUnderlyingError;
expectedUnderlying['expected-adapter']({
  path: '/tmp',
  // @ts-expect-error principal is bound by the runtime and cannot be replaced
  principal: undefined as never,
});

const mappedAdapterError = ToolInvokeError.tool(err('backend-failed', { code: 1 })).mapTool(() =>
  err('denied'),
);
const mappedAdapterCause:
  | { readonly tag: 'tool'; readonly error: ToolErr<'denied'> }
  | { readonly tag: 'invalid-tool-name'; readonly val: string }
  | { readonly tag: 'invalid-command-path'; readonly val: string[] }
  | { readonly tag: 'invalid-input'; readonly val: string }
  | { readonly tag: 'constraint-violation'; readonly val: string }
  | { readonly tag: 'invalid-result'; readonly val: string } = mappedAdapterError.cause;
void mappedAdapterCause;

const protocolMiddlewareError = new ToolInvokeError<never>({
  tag: 'invalid-input',
  val: 'bad input',
});
const protocolErrorWithMappedToolType: ToolInvokeError<ToolErr<'denied'>> =
  protocolMiddlewareError.mapTool(() => err('denied'));
void protocolErrorWithMappedToolType;

// @ts-expect-error command aliases remain metadata-only on typed underlying proxies
void (undefined as unknown as ToolUnderlying<typeof gitDef>).ci;

const aliasedMiddlewareDef = toolDefinition('aliased-middleware').command('canonical', (child) =>
  child.aliases('alias').body((body) => body.returns(z.void())),
);
aliasedMiddlewareDef.middleware({
  name: 'canonical-implementation-keys',
  implementation: {
    canonical: async () => undefined,
    // @ts-expect-error command aliases are not middleware implementation keys
    alias: async () => undefined,
  },
});

const universalMiddleware = universalToolMiddleware({
  name: 'universal-audit',
  aliases: ['audit-all'],
  doc: 'Audit every raw invocation',
  invoke: async (
    { toolName, toolMetadata, commandPath, input, stdin, principal },
    { underlying },
  ) => {
    const name: string = toolName;
    const version: string = toolMetadata.version;
    const path: readonly string[] = commandPath;
    const stream: AsyncIterable<number> | undefined = stdin;
    const sdkPrincipal: Principal = principal;
    const wireTool: WireTool = toolMetadata;
    const wireInput: WireTypedSchemaValue = input;
    const wireResult: Promise<WireInvocationResult> = underlying.invoke(
      commandPath,
      wireInput,
      stdin,
    );
    type UniversalUnderlyingErrors = Expect<
      Equal<ToolUnderlyingErrors<typeof underlying.invoke>, WireTypedSchemaValue>
    >;
    void name;
    void version;
    void path;
    void stream;
    void sdkPrincipal;
    void wireTool;
    void (undefined as unknown as UniversalUnderlyingErrors);
    // @ts-expect-error the underlying resource does not accept a replacement principal
    void underlying.invoke(commandPath, input, stdin, principal);
    return wireResult;
  },
});
const universalMiddlewareName: 'universal-audit' = universalMiddleware.name;
void universalMiddlewareName;

universalToolMiddleware({
  name: 'invalid-universal-result',
  // @ts-expect-error universal middleware must return a raw WIT invocation result
  invoke: async () => 'invalid',
});

const middlewareSubpathDefinition = middlewareEntry
  .toolDefinition('middleware-subpath')
  .body((body) => body.returns(z.string()));
middlewareSubpathDefinition.middleware({
  name: 'middleware-subpath-registration',
  implementation: {
    'middleware-subpath': async (_args, { underlying }) => underlying['middleware-subpath']({}),
  },
});
// @ts-expect-error the host-backed ambient client is not exported by the middleware subpath
void middlewareEntry.client;
// @ts-expect-error host-backed ToolCallError is not exported by the middleware subpath
void middlewareEntry.ToolCallError;
