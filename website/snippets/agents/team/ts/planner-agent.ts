export const Planner = defineAgent({
  name: 'PlannerAgent',
  id: { topic: z.string() },
  methods: { plan: method({ input: {}, returns: Plan }) },
}).implement({
  init: ({ id }) => ({ topic: id.topic }),
  methods: {
    async plan() {
      // wait for a typed result from a Rust ResearchAgent:
      const facts = await ResearchAgent.get(this.topic).gather() //@
      // send now, without waiting for a result:
      SlackAgent.get('ops').post.trigger(facts.summary) //@
      // discover agents, their methods, and available tools:
      const agents = reflection.getAllAgentTypes().map((a) => //@
        `${a.name}: ${a.methods.map((m) => m.name).join(', ')}`) //@
      const tools = reflection.getAllToolTypes().map((t) => t.name) //@
      return { facts, agents, tools }
    },
  },
})
