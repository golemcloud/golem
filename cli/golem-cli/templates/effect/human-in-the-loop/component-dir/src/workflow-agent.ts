import { Effect, Schema } from "effect";
import {
  Agents,
  defineAgent,
  Http,
  Ids,
  method,
} from "@golemcloud/effect-golem";
import { HumanAgent } from "./human-agent.js";

export const WorkflowAgent = defineAgent({
  name: "WorkflowAgent",
  id: { workflowId: Schema.String },
  http: Http.mount("/workflows/{workflowId}"),
  methods: {
    start: method({
      input: { approver: Schema.String },
      success: Schema.String,
      http: [Http.post("/start")],
    }),
  },
}).implement({
  init: ({ workflowId }) => Effect.succeed(workflowId),
  methods: (workflowId) => ({
    start: ({ approver }) =>
      Effect.gen(function* () {
        const promiseId = yield* Agents.Promises.create;
        const human = yield* HumanAgent.client.get({ username: approver });
        const requestId = yield* Schema.decodeEffect(Ids.PromiseId)(promiseId);
        yield* human.requestApproval({ workflowId, promiseId: requestId });
        const bytes = yield* Agents.Promises.await(promiseId);
        const decision = new TextDecoder().decode(bytes);
        return decision === "approved"
          ? `Workflow ${workflowId} was approved`
          : `Workflow ${workflowId} was rejected`;
      }).pipe(Effect.orDie, Effect.scoped),
  }),
});
