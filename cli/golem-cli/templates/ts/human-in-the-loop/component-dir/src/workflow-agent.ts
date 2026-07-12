import { z } from 'zod';
import { defineAgent, method, http, clientFor, createPromise, awaitPromise } from '@golemcloud/golem-ts-sdk';
import { HumanAgent, encodePromiseId } from './human-agent.js';

// A typed RPC client factory for the remote HumanAgent (wasm-RPC under the hood).
const humanClient = clientFor(HumanAgent);

// The workflow side of the loop: it creates a promise, hands it to a human for
// approval, then PAUSES until the promise is completed — the classic
// human-in-the-loop pattern. Each workflow instance gets its own generated id.
export const WorkflowAgent = defineAgent({
  name: 'WorkflowAgent',
  id: { name: z.string() },
  http: http.mount('/workflows/{name}'),
  methods: {
    start: method({ input: { approver: z.string() }, returns: z.string(), http: http.post('/start') }),
  },
});

export const WorkflowAgentImpl = WorkflowAgent.implement({
  init: () => ({ workflowId: crypto.randomUUID() as string }),
  methods: {
    async start({ approver }) {
      // 1. Create a promise that represents waiting for human input.
      const approvalPromiseId = createPromise();

      // 2. Register the pending approval with the human (remote agent call).
      //    Normally you would surface this in a UI, email, etc.
      await humanClient({ username: approver }).requestApproval({
        workflowId: this.workflowId,
        promiseId: encodePromiseId(approvalPromiseId),
      });

      // 3. Pause here until the promise is completed by the human.
      const resultBytes = await awaitPromise(approvalPromiseId);
      const decision = new TextDecoder().decode(resultBytes);

      // 4. Continue based on the human decision.
      return decision === 'approved'
        ? `Workflow ${this.workflowId} was approved`
        : `Workflow ${this.workflowId} was rejected`;
    },
  },
});
