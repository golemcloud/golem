import { z } from 'zod';
import { defineAgent, method, s, awaitPromise, createPromise, Uuid } from '@golemcloud/golem-ts-sdk';
import type { PromiseId } from 'golem:api/host@1.5.0';

const PromiseIdSchema = z.object({
  agentId: z.object({ uuid: s.uuid() }),
  fingerprint: s.uuid(),
  oplogIdx: s.u64(),
});

const toSchema = (promiseId: PromiseId) => ({
  ...promiseId,
  agentId: { ...promiseId.agentId, uuid: Uuid.from(promiseId.agentId.uuid) },
  fingerprint: Uuid.from(promiseId.fingerprint),
});

// `returnBeforeRelease` returns a promise that its unawaited background work waits for. The
// caller can complete that promise only after it receives the result, so a result that waits
// for the background work never arrives. After the release, the background work creates one
// more promise, a durable call that lands between the method's result and its settlement.
defineAgent({
  name: 'TsBackgroundWork',
  id: { name: z.string() },
  methods: {
    returnBeforeRelease: method({ input: {}, returns: PromiseIdSchema }),
  },
}).implement({
  init: () => ({}),
  methods: {
    returnBeforeRelease() {
      const release = createPromise();
      void awaitPromise(release).then(() => {
        createPromise();
      });
      return toSchema(release);
    },
  },
});
