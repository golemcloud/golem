type PlannerID struct{ Topic string }

var Planner = golem.DefineAgent[PlannerID](
    golem.Spec{Name: "PlannerAgent"})
var Plan = Planner.Method[golem.Unit, PlanResult]("plan")
var _ = Planner.Implement(func(id PlannerID) *PlannerID {
    return &id
}).Handle(Plan, golem.Adapt0(func(p *PlannerID) PlanResult {
    // use a typed client generated for a Rust ResearchAgent:
    research := researcher.MustGetResearchAgent( //@
        researcher.ResearchAgentID{Topic: p.Topic})
    facts := research.MustGather()
    // send now, without waiting for a result:
    slack.MustGetSlackAgent(slack.SlackAgentID{Channel: "ops"}).
        MustTriggerPost(facts.Summary) //@
    // schedule a call for tomorrow:
    research.MustScheduleRefresh(inDays(1)) //@
    // discover what other agents and tools offer:
    var agents, tools []string
    for _, a := range reflection.DiscoverAgentTypes() { //@
        agents = append(agents,
            fmt.Sprintf("%s (%s)", a.Name(), a.SourceLanguage()))
    }
    for _, t := range reflection.DiscoverTools() {
        tools = append(tools, t.Name())
    }
    return PlanResult{Facts: facts, Agents: agents, Tools: tools}
}))
