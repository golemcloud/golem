import {
  KeyValue,
  Path,
  c,
  client,
  command,
  compileSchema,
  ok,
  s,
  toolDefinition,
  type ToolClientTransport,
} from '@golemcloud/golem-ts-sdk';
import { typedSchemaValueToWit } from '../../src/internal/schema-model';
import { z } from 'zod';

const ArtifactRequest = z.object({
  source: z.string(),
  labels: KeyValue(z.string()),
});
const ArtifactReport = z.object({
  artifactId: s.u64(),
  digest: z.string(),
  labels: KeyValue(z.string()),
  warnings: z.array(z.string()),
});
const ValidationFailure = z.object({
  field: z.string(),
  reason: z.string(),
  retryable: z.boolean(),
});

const doc = (summary: string, description = '') => ({ summary, description, examples: [] });

export const artifactDefinition = toolDefinition('artifact')
  .version('1.0.0')
  .aliases('art')
  .doc({
    summary: 'Build and inspect artifacts',
    description: 'A deliberately asymmetric conformance tool.',
    examples: [
      {
        title: 'Render',
        body: 'artifact --region eu-west-1 render src/main.wasm --format json',
      },
    ],
  })
  .global('region', z.enum(['eu-west-1', 'us-east-1']), {
    short: 'r',
    aliases: ['location'],
    valueName: 'REGION',
    default: 'eu-west-1',
    env: 'ARTIFACT_REGION',
    doc: doc('Execution region', 'Inherited by every executable descendant.'),
  })
  .global('trace', z.boolean(), {
    kind: 'flag',
    short: 't',
    aliases: ['diagnostics'],
    negatable: true,
    doc: doc('Emit trace details'),
  })
  .command('render', (render) =>
    render
      .aliases('build')
      .doc({
        summary: 'Render one artifact',
        description: 'Build an artifact and return a structured report.',
        examples: [
          {
            title: 'Release build',
            body: 'artifact render src/main.wasm --format json --tag release --define opt=3 --checksum',
          },
        ],
      })
      .global('profile', z.enum(['debug', 'release']), {
        short: 'p',
        valueName: 'PROFILE',
        default: 'release',
        doc: doc('Build profile', 'Inherited by render descendants.'),
      })
      .annotations({ readOnly: false, destructive: false, idempotent: true, openWorld: false })
      .body((body) =>
        body
          .positional('request', ArtifactRequest, {
            valueName: 'REQUEST',
            doc: doc('Artifact request'),
          })
          .tail(
            'inputs',
            Path({ direction: 'input', kind: 'file', allowedExtensions: ['wasm', 'wat'] }),
            {
              valueName: 'INPUT',
              min: 1,
              max: 3,
              separator: '--',
              verbatim: true,
              doc: doc('Input modules', 'One to three source modules.'),
            },
          )
          .option('format', z.enum(['json', 'text']), {
            short: 'f',
            aliases: ['output-format'],
            valueName: 'FORMAT',
            default: 'json',
            doc: doc('Report format'),
          })
          .option('tag', z.string(), {
            aliases: ['label'],
            valueName: 'TAG',
            repeatable: 'either',
            delim: ',',
            default: [],
            doc: doc('Tags', 'May be repeated or comma-delimited.'),
          })
          .option('define', KeyValue(s.s64()), {
            short: 'D',
            valueName: 'KEY=VALUE',
            repeatable: 'repeated',
            default: new Map(),
            duplicateKeyPolicy: 'reject',
            doc: doc('Numeric definitions', 'Duplicate keys are rejected.'),
          })
          .option('color', z.enum(['auto', 'always', 'never']), {
            valueName: 'WHEN',
            optionalScalar: true,
            default: 'auto',
            doc: doc('Color mode', 'Bare presence resolves to the default.'),
          })
          .flag('checksum', {
            short: 'c',
            aliases: ['digest'],
            negatable: true,
            doc: doc('Include digest'),
          })
          .flag('verbose', {
            kind: 'count-flag',
            short: 'v',
            max: 3,
            doc: doc('Verbosity'),
          })
          .constraint(c.requiresAll([c.present('checksum'), c.present('format')]))
          .constraint(
            c.implies({
              lhs: c.valueIs('profile', 'release'),
              rhs: c.present('tag'),
              rhsQuant: 'any',
            }),
          )
          .constraint(
            c.forbids({
              lhs: c.valueIs('format', 'text'),
              rhs: c.present('define'),
              lhsQuant: 'any',
            }),
          )
          .stdin({
            required: false,
            mime: ['application/wasm'],
            doc: doc('Optional module bytes'),
          })
          .stdout({
            required: true,
            mime: ['text/plain; charset=utf-8'],
            doc: doc('Progress output'),
          })
          .stderr({
            required: false,
            mime: ['application/octet-stream'],
            doc: doc('Diagnostic bytes'),
          })
          .returns(ArtifactReport, {
            formatters: [
              { name: 'json', doc: doc('JSON report') },
              { name: 'table', doc: doc('Tabular report') },
            ],
            defaultFormatter: 'json',
            doc: doc('Artifact report'),
          })
          .error('invalid-request', {
            kind: 'usage',
            exitCode: 2,
            payload: ValidationFailure,
            doc: doc('Request validation failed'),
          })
          .error('render-failed', {
            kind: 'runtime',
            exitCode: 70,
            payload: z.object({ stage: z.string(), code: s.u32() }),
            doc: doc('Renderer failed'),
          }),
      )
      .command('status', (status) =>
        status
          .aliases('show')
          .doc(doc('Inspect render status'))
          .annotations({ readOnly: true, destructive: false, idempotent: true, openWorld: false })
          .body((body) =>
            body
              .positional('artifact-id', s.u64(), {
                valueName: 'ID',
                doc: doc('Artifact identifier'),
              })
              .returns(z.enum(['queued', 'ready', 'failed']), {
                formatters: [{ name: 'json', doc: doc('JSON status') }],
                defaultFormatter: 'json',
                doc: doc('Current status'),
              }),
          ),
      ),
  );

const wire = (schema: Parameters<typeof compileSchema>[0], value: unknown) => {
  const codec = compileSchema(schema);
  return typedSchemaValueToWit({ graph: codec.graph, value: codec.toValue(value) });
};

export interface ConformanceObservation {
  readonly path: readonly string[];
  readonly input: unknown;
  readonly stdin: ReadableStream<Uint8Array> | undefined;
  readonly stdout: boolean;
  readonly stderr: boolean;
}

export interface ConformanceTransportState {
  failNext: boolean;
}

export function conformanceTransport(
  observations: ConformanceObservation[],
  state: ConformanceTransportState = { failNext: false },
): ToolClientTransport {
  return {
    start(path, input, stdin, stdout, stderr) {
      observations.push({ path: [...path], input, stdin, stdout, stderr });
      if (path.join('/') === 'render/status') {
        const result = {
          ...wire(z.enum(['queued', 'ready', 'failed']), 'ready'),
          value: { valueNodes: [{ tag: 'enum-value' as const, val: 1 }], root: 0 },
        };
        return {
          settledResult: Promise.resolve({
            status: 'fulfilled',
            value: { result },
          }),
          cancel() {},
        };
      }
      const fail = state.failNext;
      state.failNext = false;
      return {
        settledResult: fail
          ? Promise.resolve({
              status: 'rejected' as const,
              reason: {
                tag: 'remote-tool-error' as const,
                val: {
                  tag: 'custom-error' as const,
                  val: {
                    name: 'invalid-request',
                    payload: wire(ValidationFailure, {
                      field: 'request.source',
                      reason: 'unsupported module',
                      retryable: false,
                    }),
                  },
                },
              },
            })
          : Promise.resolve({
              status: 'fulfilled' as const,
              value: {
                result: wire(ArtifactReport, {
                  artifactId: 18446744073709551614n,
                  digest: 'deadbeef',
                  labels: new Map([
                    ['tier', 'gold'],
                    ['team', 'runtime'],
                  ]),
                  warnings: ['unsigned metadata'],
                }),
              },
            }),
        stdout: (async function* () {
          yield { tag: 'ok' as const, val: new TextEncoder().encode('compiled 2 inputs\n') };
        })(),
        stderr: (async function* () {
          yield { tag: 'ok' as const, val: Uint8Array.of(0, 1, 2) };
        })(),
        cancel() {},
      };
    },
  };
}

const observations: ConformanceObservation[] = [];
const state = { failNext: false };
const transport = conformanceTransport(observations, state);
const definitionOwned = Reflect.apply(artifactDefinition.client, artifactDefinition, [
  { transport },
]);
const generated = client(artifactDefinition, { transport });

artifactDefinition.implement({
  render: command(
    () =>
      ok({
        artifactId: 18446744073709551614n,
        digest: 'deadbeef',
        labels: new Map([
          ['tier', 'gold'],
          ['team', 'runtime'],
        ]),
        warnings: ['unsigned metadata'],
      }),
    { status: () => ok('ready') },
  ),
});

Object.assign(globalThis, {
  __golemGol40TypeScriptConformance: {
    definitionOwned,
    generated,
    observations,
    transport,
    failNext() {
      state.failNext = true;
    },
  },
});
