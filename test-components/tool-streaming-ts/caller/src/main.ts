import {
  acquireQuotaToken,
  AgentStream,
  client,
  defineAgent,
  method,
  Result,
  s,
  toolDefinition,
  ToolStreamError,
  type PermissionCard,
} from '@golemcloud/golem-ts-sdk';
import { MatrixCoreClient } from 'matrix-core-tool-guest-client';
import { MatrixResourceClient } from 'matrix-resource-tool-guest-client';
import { TsStreamingClient } from 'ts-streaming-tool-guest-client';
import { getConfigValue } from 'golem:agent/host@2.0.0';
import type { SchemaGraph, Secret } from 'golem:core/types@2.0.0';
import { z } from 'zod/v4';
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { rm } from 'node:fs/promises';

const TscClient = client(
  toolDefinition('tsc', { requiresFilesystem: true })
    .version('5.9.2+golem.4')
    .body((body) =>
      body
        .option('cwd', z.string(), { default: '/workspace' })
        .tail('args', z.string(), { separator: '--', verbatim: true })
        .stdout({ required: true })
        .stderr({ required: true })
        .returns(s.s32()),
    ),
);

const MatrixCoreObservation = z.object({
  provider: z.string(),
  command: z.string(),
  normalizedSource: z.string(),
  weightedSize: s.s64(),
  labelSummary: z.string(),
  principal: z.string(),
  ownerAgentId: z.string(),
  errorField: z.string(),
  errorReason: z.string(),
  errorRetryable: z.boolean(),
});

const MatrixResourceObservation = z.object({
  secretFirstProvider: z.string(),
  secretSecondProvider: z.string(),
  secretFirstRevealed: z.boolean(),
  secretSecondRevealed: z.boolean(),
  secretPrincipal: z.string(),
  secretOwnerAgentId: z.string(),
  quotaProvider: z.string(),
  quotaReserved: z.boolean(),
  quotaReturnedUsable: z.boolean(),
  quotaOriginalConsumed: z.boolean(),
  quotaPrincipal: z.string(),
  quotaOwnerAgentId: z.string(),
  permissionSupported: z.boolean(),
  permissionProvider: z.string(),
  permissionSameIdentity: z.boolean(),
  permissionOriginalConsumed: z.boolean(),
  permissionPrincipal: z.string(),
  permissionOwnerAgentId: z.string(),
  typedValues: z.array(s.u32() as unknown as z.ZodType<number, number>),
});

const PermissionCardSchema = s.permissionCard({
  polymorphic: false,
}) as unknown as z.ZodType<PermissionCard, PermissionCard>;

const MatrixPermissionIssuerClient = client(
  toolDefinition('matrix-permission-issuer')
    .version('1.0.0')
    .command('issue', (issue) =>
      issue.body((body) =>
        body.returns(
          z.object({
            card: PermissionCardSchema,
            issuer: z.string(),
            principal: z.string(),
            ownerAgentId: z.string(),
          }),
        ),
      ),
    ),
  { lookupName: 'matrix-permission-issuer' },
);

function emptyResourceObservation(): z.infer<typeof MatrixResourceObservation> {
  return {
    secretFirstProvider: '',
    secretSecondProvider: '',
    secretFirstRevealed: false,
    secretSecondRevealed: false,
    secretPrincipal: '',
    secretOwnerAgentId: '',
    quotaProvider: '',
    quotaReserved: false,
    quotaReturnedUsable: false,
    quotaOriginalConsumed: false,
    quotaPrincipal: '',
    quotaOwnerAgentId: '',
    permissionSupported: false,
    permissionProvider: '',
    permissionSameIdentity: false,
    permissionOriginalConsumed: false,
    permissionPrincipal: '',
    permissionOwnerAgentId: '',
    typedValues: [],
  };
}

const SECRET_STRING_GRAPH: SchemaGraph = {
  typeNodes: [
    {
      body: { tag: 'string-type' },
      metadata: { aliases: [], examples: [] },
    },
    {
      body: { tag: 'secret-type', val: { inner: 0 } },
      metadata: { aliases: [], examples: [] },
    },
  ],
  defs: [],
  root: 1,
};

function configuredSecret(): Secret {
  const value = getConfigValue(['secret'], SECRET_STRING_GRAPH);
  const root = value.valueNodes[value.root];
  if (root?.tag !== 'secret-value') {
    throw new Error("config path 'secret' did not resolve to secret<string>");
  }
  return root.val;
}

const Evidence = z.object({
  output: s.bytes(),
  bytesRead: s.u64(),
});

const CompletionEvidence = z.object({
  output: s.bytes(),
  stdoutTerminal: z.string(),
  resultTerminal: z.string(),
});

const DualOutputEvidence = z.object({
  stdout: s.bytes(),
  stderr: s.bytes(),
  resultTerminal: z.string(),
});

async function collect(reader: ReadableStreamDefaultReader<Uint8Array>): Promise<Uint8Array> {
  const chunks: Uint8Array[] = [];
  while (true) {
    const item = await reader.read();
    if (item.done) break;
    chunks.push(item.value);
  }
  const result = new Uint8Array(chunks.reduce((size, chunk) => size + chunk.byteLength, 0));
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return result;
}

const Caller = defineAgent({
  name: 'TsToolStreamingCaller',
  id: { name: z.string() },
  methods: {
    recursiveRmToolOutput: method({
      input: { promises: z.boolean() },
      returns: s.result(z.string(), z.string()),
    }),
    markerBeforeEof: method({
      input: { payload: s.bytes() },
      returns: Evidence,
    }),
    typedStdoutFailure: method({
      input: {},
      returns: z.string(),
    }),
    declaredErrorCompletion: method({
      input: {},
      returns: CompletionEvidence,
    }),
    dualOutputDeclaredError: method({
      input: {},
      returns: DualOutputEvidence,
    }),
    matrix_core_observation: method({
      input: {},
      returns: MatrixCoreObservation,
    }),
  },
});

const ResourceCaller = defineAgent({
  name: 'TsResourceToolStreamingCaller',
  id: { name: z.string() },
  config: {
    secret: s.secret(z.string()),
  },
  methods: {
    matrix_secret_observation: method({
      input: {},
      returns: MatrixResourceObservation,
    }),
    matrix_quota_observation: method({
      input: {},
      returns: MatrixResourceObservation,
    }),
    matrix_permission_observation: method({
      input: {},
      returns: MatrixResourceObservation,
    }),
    matrix_typed_stream_observation: method({
      input: {},
      returns: MatrixResourceObservation,
    }),
    matrix_resource_observation: method({
      input: {},
      returns: MatrixResourceObservation,
    }),
  },
});

ResourceCaller.implement({
  init: () => ({}),
  methods: {
    async matrix_secret_observation() {
      const client = MatrixResourceClient.newClient();
      const secretFirst = await client.secret().exchange(configuredSecret());
      const secretSecond = await client.secret().exchange(secretFirst.secret);
      return {
        ...emptyResourceObservation(),
        secretFirstProvider: secretFirst.provider,
        secretSecondProvider: secretSecond.provider,
        secretFirstRevealed: secretFirst.revealed,
        secretSecondRevealed: secretSecond.revealed,
        secretPrincipal: secretSecond.principal,
        secretOwnerAgentId: secretSecond.ownerAgentId,
      };
    },
    async matrix_quota_observation() {
      const originalQuota = acquireQuotaToken('matrix-capacity', 2n);
      const quota = await MatrixResourceClient.newClient().quota().exchange(originalQuota);
      let quotaOriginalConsumed = false;
      try {
        originalQuota.reserve(0n).unwrap().commit(0n);
      } catch {
        quotaOriginalConsumed = true;
      }
      let quotaReturnedUsable = false;
      try {
        quota.token.reserve(0n).unwrap().commit(0n);
        quotaReturnedUsable = true;
      } catch {
        quotaReturnedUsable = false;
      }
      return {
        ...emptyResourceObservation(),
        quotaProvider: quota.provider,
        quotaReserved: quota.reserved,
        quotaReturnedUsable,
        quotaOriginalConsumed,
        quotaPrincipal: quota.principal,
        quotaOwnerAgentId: quota.ownerAgentId,
      };
    },
    async matrix_permission_observation() {
      const issued = await MatrixPermissionIssuerClient.issue({});
      if (issued.issuer !== 'rust') {
        throw new Error(`unexpected matrix permission issuer: ${issued.issuer}`);
      }
      const originalCard: PermissionCard = issued.card;
      const permission = await MatrixResourceClient.newClient()
        .permissions()
        .exchange(originalCard);
      if (
        issued.principal !== permission.principal ||
        issued.ownerAgentId !== permission.ownerAgentId
      ) {
        throw new Error('matrix permission issuer evidence changed in transit');
      }
      let permissionOriginalConsumed = false;
      try {
        await MatrixResourceClient.newClient().permissions().exchange(originalCard);
      } catch {
        permissionOriginalConsumed = true;
      }
      const permissionSameIdentity = permissionOriginalConsumed && permission.card !== undefined;
      return {
        ...emptyResourceObservation(),
        permissionSupported: true,
        permissionProvider: permission.provider,
        permissionSameIdentity,
        permissionOriginalConsumed,
        permissionPrincipal: permission.principal,
        permissionOwnerAgentId: permission.ownerAgentId,
      };
    },
    async matrix_typed_stream_observation() {
      const typedValues: number[] = [];
      const transformed = await MatrixResourceClient.newClient()
        .typed()
        .transform(AgentStream.from([2, 5, 9]));
      for await (const value of transformed) typedValues.push(value);
      return { ...emptyResourceObservation(), typedValues };
    },
    async matrix_resource_observation() {
      const client = MatrixResourceClient.newClient();
      const secretFirst = await client.secret().exchange(configuredSecret());
      const secretSecond = await client.secret().exchange(secretFirst.secret);

      const originalQuota = acquireQuotaToken('matrix-capacity', 2n);
      const quota = await client.quota().exchange(originalQuota);
      let quotaOriginalConsumed = false;
      try {
        originalQuota.reserve(0n).unwrap().commit(0n);
      } catch {
        quotaOriginalConsumed = true;
      }
      let quotaReturnedUsable = false;
      try {
        quota.token.reserve(0n).unwrap().commit(0n);
        quotaReturnedUsable = true;
      } catch {
        quotaReturnedUsable = false;
      }

      return {
        secretFirstProvider: secretFirst.provider,
        secretSecondProvider: secretSecond.provider,
        secretFirstRevealed: secretFirst.revealed,
        secretSecondRevealed: secretSecond.revealed,
        secretPrincipal: secretFirst.principal,
        secretOwnerAgentId: secretFirst.ownerAgentId,
        quotaProvider: quota.provider,
        quotaReserved: quota.reserved,
        quotaReturnedUsable,
        quotaOriginalConsumed,
        quotaPrincipal: quota.principal,
        quotaOwnerAgentId: quota.ownerAgentId,
        permissionSupported: false,
        permissionProvider: '',
        permissionSameIdentity: false,
        permissionOriginalConsumed: false,
        permissionPrincipal: '',
        permissionOwnerAgentId: '',
        typedValues: [],
      };
    },
  },
});

Caller.implement({
  init: () => ({}),
  methods: {
    async recursiveRmToolOutput({ promises }) {
      const root = '/workspace/recursive-rm';
      const candidate = `${root}/candidate`;
      mkdirSync(`${root}/src/nested`, { recursive: true });
      writeFileSync(`${root}/src/main.ts`, 'export const main: number = 17;\n');
      writeFileSync(`${root}/src/nested/value.ts`, 'export const value: number = 42;\n');
      const compile = async () => {
        const invocation = TscClient.tsc({
          cwd: root,
          args: [
            '--pretty',
            'false',
            '--target',
            'es2022',
            '--module',
            'es2022',
            '--rootDir',
            'src',
            '--outDir',
            'candidate',
            '--noEmitOnError',
            'src/main.ts',
            'src/nested/value.ts',
          ],
        });
        const [exitCode, stdout, stderr] = await Promise.all([
          invocation.result,
          collect(invocation.stdout!.getReader()),
          collect(invocation.stderr!.getReader()),
        ]);
        if (exitCode !== 0 || stdout.length !== 0 || stderr.length !== 0) {
          throw new Error(
            `tsc: ${exitCode}: ${new TextDecoder().decode(stdout)} ${new TextDecoder().decode(stderr)}`,
          );
        }
        if (
          readFileSync(`${candidate}/main.js`, 'utf8') !== 'export const main = 17;\n' ||
          readFileSync(`${candidate}/nested/value.js`, 'utf8') !== 'export const value = 42;\n'
        ) {
          throw new Error('compiler output differs');
        }
      };
      await compile();
      try {
        if (promises) await rm(candidate, { recursive: true, force: true });
        else rmSync(candidate, { recursive: true, force: true });
      } catch (error) {
        return Result.err(`readable compiler output could not be removed: ${String(error)}`);
      }
      if (
        existsSync(candidate) ||
        readFileSync(`${root}/src/nested/value.ts`, 'utf8') !== 'export const value: number = 42;\n'
      ) {
        throw new Error('removal left output behind or changed the source');
      }
      await compile();
      return Result.ok('removed-output-source-preserved-recompiled');
    },
    async matrix_core_observation() {
      const client = MatrixCoreClient.newClient();
      const success = await client.artifact().inspect(
        {
          source: 'matrix.sample',
          dimensions: { width: 3, height: 5 },
          labels: ['north', 'east', 'south'],
        },
        7n,
      );

      try {
        await client.artifact().inspect(
          {
            source: 'reject.me',
            dimensions: { width: 13, height: 5 },
            labels: ['unused', 'error'],
          },
          2n,
        );
        throw new Error('matrix-core reject.me unexpectedly succeeded');
      } catch (error) {
        const declared = error as {
          tag?: unknown;
          error?: {
            tag?: unknown;
            value?: { field: string; reason: string; retryable: boolean };
          };
        };
        if (
          declared.tag !== 'tool' ||
          declared.error?.tag !== 'Rejected' ||
          declared.error.value === undefined
        ) {
          throw error;
        }
        const payload = declared.error.value;
        return {
          ...success,
          errorField: payload.field,
          errorReason: payload.reason,
          errorRetryable: payload.retryable,
        };
      }
    },
    async dualOutputDeclaredError() {
      const invocation = TsStreamingClient.newClient().dual();
      if (!invocation.stdout || !invocation.stderr) {
        throw new Error('TypeScript dual-output invocation omitted a declared channel');
      }
      const [result, stdout, stderr] = await Promise.all([
        invocation.result.then(
          () => 'ok',
          () => 'declared-error',
        ),
        collect(invocation.stdout.getReader()),
        collect(invocation.stderr.getReader()),
      ]);
      return { stdout, stderr, resultTerminal: result };
    },
    async markerBeforeEof({ payload }) {
      let releaseInput!: () => void;
      const inputGate = new Promise<void>((resolve) => {
        releaseInput = resolve;
      });
      const stdin = new ReadableStream<Uint8Array>({
        async start(controller) {
          await inputGate;
          controller.enqueue(new Uint8Array());
          if (payload.byteLength > 0) controller.enqueue(payload);
          controller.close();
        },
      });
      const invocation = TsStreamingClient.newClient().ts_streaming('marker-echo', stdin);
      if (!invocation.stdout) {
        throw new Error('TypeScript tool invocation omitted declared stdout');
      }
      const reader = invocation.stdout.getReader();
      const marker = await reader.read();
      if (marker.done || new TextDecoder().decode(marker.value) !== 'ts-marker:') {
        throw new Error('TypeScript tool stdout marker was not live before stdin EOF');
      }

      releaseInput();
      const chunks = [marker.value];
      const [bytesRead] = await Promise.all([
        invocation.result,
        (async () => {
          while (true) {
            const item = await reader.read();
            if (item.done) break;
            chunks.push(item.value);
          }
        })(),
      ]);
      const output = new Uint8Array(chunks.reduce((size, chunk) => size + chunk.byteLength, 0));
      let offset = 0;
      for (const chunk of chunks) {
        output.set(chunk, offset);
        offset += chunk.byteLength;
      }
      return { output, bytesRead };
    },
    async typedStdoutFailure() {
      const stdin = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.close();
        },
      });
      const invocation = TsStreamingClient.newClient().ts_streaming('resource-exhausted', stdin);
      if (!invocation.stdout) {
        throw new Error('TypeScript tool invocation omitted declared stdout');
      }
      const [result, stdout] = await Promise.allSettled([
        invocation.result,
        invocation.stdout.getReader().read(),
      ]);
      if (result.status === 'rejected') throw result.reason;
      if (result.value !== 0n) {
        throw new Error(`Unexpected structured result ${result.value}`);
      }
      if (stdout.status === 'fulfilled') {
        throw new Error('Expected typed stdout failure');
      }
      if (!(stdout.reason instanceof ToolStreamError)) throw stdout.reason;
      return stdout.reason.failure.tag;
    },
    async declaredErrorCompletion() {
      const stdin = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.close();
        },
      });
      const invocation = TsStreamingClient.newClient().ts_streaming('declared-error', stdin);
      if (!invocation.stdout) {
        throw new Error('TypeScript tool invocation omitted declared stdout');
      }
      const chunks: Uint8Array[] = [];
      const reader = invocation.stdout.getReader();
      const [result, stdout] = await Promise.allSettled([
        invocation.result,
        (async () => {
          while (true) {
            const item = await reader.read();
            if (item.done) return 'finished';
            chunks.push(item.value);
          }
        })(),
      ]);
      const output = new Uint8Array(chunks.reduce((size, chunk) => size + chunk.byteLength, 0));
      let offset = 0;
      for (const chunk of chunks) {
        output.set(chunk, offset);
        offset += chunk.byteLength;
      }
      return {
        output,
        stdoutTerminal: stdout.status === 'fulfilled' ? stdout.value : 'failed',
        resultTerminal: result.status === 'rejected' ? 'declared-error' : 'ok',
      };
    },
  },
});
