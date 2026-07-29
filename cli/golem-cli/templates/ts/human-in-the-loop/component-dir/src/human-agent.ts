import { z } from 'zod';
import { defineAgent, method, http, completePromise, PromiseId } from '@golemcloud/golem-ts-sdk';

export type WorkflowId = string;

// A Golem `PromiseId` is a nested record carrying bigints (its component-id halves
// and oplog index), so instead of declaring a matching schema we ship it across
// the agent boundary as a bigint-aware JSON string. The awaiting WorkflowAgent
// encodes the id it created; the HumanAgent decodes it to complete the promise.
export function encodePromiseId(id: PromiseId): string {
  return JSON.stringify(id, (_key, value) =>
    typeof value === 'bigint' ? { '#bigint': value.toString() } : value,
  );
}

export function decodePromiseId(text: string): PromiseId {
  return JSON.parse(text, (_key, value) =>
    value && typeof value === 'object' && '#bigint' in value
      ? BigInt((value as { '#bigint': string })['#bigint'])
      : value,
  ) as PromiseId;
}

// The human side of the loop: it collects pending approval requests and, when a
// decision is made, completes the workflow's promise — unblocking the paused
// WorkflowAgent. `requestApproval` is called agent-to-agent; the other two are
// also exposed over HTTP so a UI can drive the human's decisions.
export const HumanAgent = defineAgent({
  name: 'HumanAgent',
  id: { username: z.string() },
  http: http.mount('/humans/{username}'),
  methods: {
    requestApproval: method({
      input: { workflowId: z.string(), promiseId: z.string() },
      returns: z.string(),
    }),
    listPendingApprovals: method({
      input: {},
      returns: z.array(z.string()),
      http: http.get('/pending'),
    }),
    decideApproval: method({
      input: { workflowId: z.string(), decision: z.string() },
      returns: z.string(),
      http: http.post('/decisions'),
    }),
  },
});

export const HumanAgentImpl = HumanAgent.implement({
  init: ({ id }) => ({ username: id.username, pending: new Map<WorkflowId, string>() }),
  methods: {
    requestApproval({ workflowId, promiseId }) {
      this.pending.set(workflowId, promiseId);
      return `User ${this.username} received approval request for workflow ${workflowId}`;
    },
    listPendingApprovals() {
      return Array.from(this.pending.keys());
    },
    decideApproval({ workflowId, decision }) {
      if (!['approved', 'rejected'].includes(decision)) {
        return `Received invalid approval decision ${decision}`;
      }

      const promiseId = this.pending.get(workflowId);
      if (!promiseId) {
        return `No pending request found for workflow ${workflowId}`;
      }

      completePromise(decodePromiseId(promiseId), new TextEncoder().encode(decision));
      this.pending.delete(workflowId);

      return `Workflow ${workflowId} was ${decision} by ${this.username}`;
    },
  },
});
