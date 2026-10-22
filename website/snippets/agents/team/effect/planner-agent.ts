export const Planner = defineAgent({
  name: "PlannerAgent",
  id: { topic: Schema.String },
  methods: { plan: method({ input: {}, success: Plan }) },
}).implement({
  init: ({ topic }) => Effect.succeed({ topic }),
  methods: ({ topic }) => ({
    plan: () => Effect.gen(function* () {
      // wait for a typed result from a Rust ResearchAgent:
      const research = yield* ResearchAgent.get(topic) //@
      const facts = yield* research.gather() //@
      // send now, without waiting for a result:
      const slack = yield* SlackAgent.get("ops")
      yield* slack.post.trigger(facts.summary) //@
      // discover agents, their methods, and available tools:
      const agents = (yield* Reflection.getAllAgentTypes).map((a) => //@
        `${a.name}: ${a.methods.map((m) => m.name).join(", ")}`) //@
      const tools = (yield* Reflection.getAllToolTypes()) //@
        .map((t) => t.name)
      return { facts, agents, tools }
    }).pipe(Effect.scoped, Effect.orDie),
  }),
})
