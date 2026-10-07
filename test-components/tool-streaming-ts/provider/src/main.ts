import {
  AgentStream,
  command,
  err,
  getSelfMetadata,
  ok,
  type PermissionCard,
  type Principal,
  s,
  toolDefinition,
  ToolStreamError,
} from '@golemcloud/golem-ts-sdk';
import type { QuotaToken, SchemaGraph, Secret } from 'golem:core/types@2.0.0';
import { Reservation, reserve } from 'golem:quota/types@1.5.0';
import { reveal } from 'golem:secrets/reveal@0.1.0';
import { z } from 'zod/v4';

const U32 = s.u32() as unknown as z.ZodType<number, number>;

const MatrixRequest = z.object({
  source: z.string(),
  dimensions: z.object({
    width: U32,
    height: U32,
  }),
  labels: z.array(z.string()),
});

const MatrixResult = z.object({
  provider: z.string(),
  command: z.string(),
  normalizedSource: z.string(),
  weightedSize: s.s64(),
  labelSummary: z.string(),
  principal: z.string(),
  ownerAgentId: z.string(),
});

const MatrixRejection = z.object({
  field: z.string(),
  reason: z.string(),
  retryable: z.boolean(),
});

function principalName(principal: Principal): string {
  return principal.tag === 'oidc' ? `oidc:${principal.sub}` : principal.tag;
}

const SecretExchange = z.object({
  provider: z.string(),
  principal: z.string(),
  ownerAgentId: z.string(),
  revealed: z.boolean(),
  secret: s.secret(z.string()),
});

const QuotaExchange = z.object({
  provider: z.string(),
  principal: z.string(),
  ownerAgentId: z.string(),
  reserved: z.boolean(),
  token: s.quotaToken(),
});

const PermissionExchange = z.object({
  provider: z.string(),
  principal: z.string(),
  ownerAgentId: z.string(),
  card: s.permissionCard({ polymorphic: false }),
});

const STRING_GRAPH: SchemaGraph = {
  typeNodes: [
    {
      body: { tag: 'string-type' },
      metadata: { aliases: [], examples: [] },
    },
  ],
  defs: [],
  root: 0,
};

function revealString(secret: Secret): string {
  const revealed = reveal(secret, STRING_GRAPH);
  const value = revealed.valueNodes[revealed.root];
  if (value?.tag !== 'string-value') {
    throw new Error('matrix secret did not reveal as a string');
  }
  return value.val;
}

function resourceEvidence(context: { principal: Principal }) {
  return {
    provider: 'typescript',
    principal: principalName(context.principal),
    ownerAgentId: getSelfMetadata().agentId.agentId,
  };
}

toolDefinition('matrix-resource')
  .version('1.0.0')
  .command('secret', (secret) =>
    secret.command('exchange', (exchange) =>
      exchange.body((body) =>
        body.positional('secret', s.secret(z.string())).returns(SecretExchange),
      ),
    ),
  )
  .command('quota', (quota) =>
    quota.command('exchange', (exchange) =>
      exchange.body((body) => body.positional('token', s.quotaToken()).returns(QuotaExchange)),
    ),
  )
  .command('permissions', (permissions) =>
    permissions.command('exchange', (exchange) =>
      exchange.body((body) =>
        body
          .positional('card', s.permissionCard({ polymorphic: false }))
          .returns(PermissionExchange),
      ),
    ),
  )
  .command('typed', (typed) =>
    typed.command('transform', (transform) =>
      transform.body((body) =>
        body.positional('input', s.stream(s.u32())).returns(s.stream(s.u32())),
      ),
    ),
  )
  .implement({
    secret: command({
      exchange: async ({ secret }: { secret: Secret }, context) =>
        ok({
          ...resourceEvidence(context),
          revealed: revealString(secret) === 'matrix-secret-value',
          secret,
        }),
    }),
    quota: command({
      exchange: async ({ token }: { token: QuotaToken }, context) => {
        let reserved = false;
        try {
          Reservation.commit(reserve(token, 1n), 1n);
          reserved = true;
        } catch {
          reserved = false;
        }
        return ok({ ...resourceEvidence(context), reserved, token });
      },
    }),
    permissions: command({
      exchange: async ({ card }: { card: PermissionCard }, context) =>
        ok({ ...resourceEvidence(context), card }),
    }),
    typed: command({
      transform: async ({ input }: { input: AgentStream<number> }) =>
        ok(
          AgentStream.from(
            (async function* () {
              for await (const value of input) {
                yield value * 3 + 1;
              }
            })(),
          ),
        ),
    }),
  });

toolDefinition('matrix-core')
  .version('1.0.0')
  .command('artifact', (artifact) =>
    artifact.command('inspect', (inspect) =>
      inspect.body((body) =>
        body
          .positional('request', MatrixRequest)
          .positional('multiplier', s.s64())
          .returns(MatrixResult)
          .error('rejected', {
            kind: 'usage',
            exitCode: 2,
            payload: MatrixRejection,
          }),
      ),
    ),
  )
  .implement({
    artifact: command({
      inspect: async ({ request, multiplier }, context) => {
        if (request.source === 'reject.me') {
          return err('rejected', {
            field: 'request.source',
            reason: 'unsupported source',
            retryable: false,
          });
        }

        return ok({
          provider: 'typescript',
          command: 'artifact/inspect',
          normalizedSource: request.source.toUpperCase(),
          weightedSize:
            BigInt(request.dimensions.width) * BigInt(request.dimensions.height) * multiplier +
            BigInt(request.labels.length),
          labelSummary: [...request.labels].reverse().join('|'),
          principal: principalName(context.principal),
          ownerAgentId: getSelfMetadata().agentId.agentId,
        });
      },
    }),
  });

toolDefinition('ts-streaming')
  .body((body) =>
    body
      .positional('mode', z.string())
      .stdin({ required: true })
      .stdout({ required: true })
      .returns(s.u64())
      .error('declared', {
        kind: 'runtime',
        exitCode: 1,
        payload: z.string(),
      }),
  )
  .command('dual', (dual) =>
    dual.body((body) =>
      body
        .stdout({ required: true })
        .stderr({ required: true })
        .returns(z.string())
        .error('declared', {
          kind: 'runtime',
          exitCode: 1,
          payload: z.string(),
        }),
    ),
  )
  .implement({
    'ts-streaming': async ({ mode }, context) => {
      const reader = context.stdin.getReader();
      const writer = context.stdout.getWriter();
      let bytesRead = 0n;

      if (mode === 'resource-exhausted') {
        await writer.abort(new ToolStreamError({ tag: 'resource-exhausted' }));
        return ok(bytesRead);
      }

      if (mode === 'marker-echo') {
        await writer.write(new Uint8Array());
        await writer.write(new TextEncoder().encode('ts-marker:'));
      }

      if (mode === 'declared-error') {
        await writer.write(new TextEncoder().encode('ts-declared:'));
      }

      while (true) {
        const item = await reader.read();
        if (item.done) break;
        bytesRead += BigInt(item.value.byteLength);
        await writer.write(item.value);
      }

      return mode === 'declared-error' ? err('declared', 'expected') : ok(bytesRead);
    },
    dual: async (_input, context) => {
      const stdout = context.stdout.getWriter();
      const stderr = context.stderr.getWriter();
      await Promise.all([
        stdout.write(new Uint8Array([0, 127, 128, 255])),
        stderr.write(new Uint8Array([255, 128, 1, 2])),
      ]);
      return err('declared', 'dual-expected');
    },
  });
