export const Planner = defineAgent({
  name: 'PlannerAgent',
  id: { topic: z.string() },
  methods: { plan: method({ input: {}, returns: Plan }) },
}).implement({
  init: ({ id }) => ({ topic: id.topic }),
  methods: {
    async plan() {
      // use a typed client generated for a Rust ResearchAgent:
      const research = ResearchAgent.get(this.topic) //@
      const facts = await research.gather()
      // send now, without waiting for a result:
      SlackAgent.get('ops').post.trigger(facts.summary) //@
      // schedule a call for tomorrow:
      research.refresh.schedule(inDays(1)) //@
      // discover what other agents and tools offer:
      const agents = reflection.getAllAgentTypes() //@
        .map((a) => `${a.name} (${a.sourceLanguage})`)
      const tools = reflection.getAllToolTypes().map((t) => t.name)
      return { facts, agents, tools }
    },
  },
})
