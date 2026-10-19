import { Effect, Schema } from "effect";
import {
  Agents,
  defineAgent,
  Http,
  Ids,
  method,
} from "@golemcloud/effect-golem";

export const HumanAgent = defineAgent({
  name: "HumanAgent",
  id: { username: Schema.String },
  http: Http.mount("/humans/{username}"),
  methods: {
    requestApproval: method({
      input: { workflowId: Schema.String, promiseId: Ids.PromiseId },
      success: Schema.String,
    }),
    listPendingApprovals: method({
      input: {},
      success: Schema.Array(Schema.String),
      http: [Http.get("/pending")],
    }),
    decideApproval: method({
      input: { workflowId: Schema.String, decision: Schema.String },
      success: Schema.String,
      http: [Http.post("/decisions")],
    }),
  },
}).implement({
  init: ({ username }) =>
    Effect.succeed({
      username,
      pending: new Map<string, Agents.PromiseId>(),
    }),
  methods: ({ username, pending }) => ({
    requestApproval: ({ workflowId, promiseId }) =>
      Effect.gen(function* () {
        const nativeId = yield* Schema.encodeEffect(Ids.PromiseId)(promiseId);
        pending.set(workflowId, nativeId);
        return `User ${username} received approval request for workflow ${workflowId}`;
      }).pipe(Effect.orDie),
    listPendingApprovals: () => Effect.sync(() => Array.from(pending.keys())),
    decideApproval: ({ workflowId, decision }) =>
      Effect.gen(function* () {
        if (decision !== "approved" && decision !== "rejected") {
          return `Received invalid approval decision ${decision}`;
        }
        const promiseId = pending.get(workflowId);
        if (!promiseId)
          return `No pending request found for workflow ${workflowId}`;
        yield* Agents.Promises.complete(
          promiseId,
          new TextEncoder().encode(decision),
        );
        pending.delete(workflowId);
        return `Workflow ${workflowId} was ${decision} by ${username}`;
      }).pipe(Effect.orDie),
  }),
});
