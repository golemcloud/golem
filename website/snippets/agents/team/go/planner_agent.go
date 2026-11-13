type PlannerID struct{ Topic string }

var Planner = golem.DefineAgent[PlannerID](
    golem.Spec{Name: "PlannerAgent"})
var Plan = Planner.Method[golem.Unit, PlanResult]("plan")
var _ = Planner.Implement(func(id PlannerID) *PlannerID {
    return &id
}).Handle(Plan, golem.Adapt0(func(p *PlannerID) PlanResult {
    // wait for a typed result from a Rust ResearchAgent:
    facts := researcher.MustGetResearchAgent( //@
        researcher.ResearchAgentID{Topic: p.Topic}).MustGather() //@
    // send now, without waiting for a result:
    slack.MustGetSlackAgent(slack.SlackAgentID{Channel: "ops"}).
        MustTriggerPost(facts.Summary) //@
    // discover agents, their methods, and available tools:
    var agents, tools []string
    for _, a := range reflection.DiscoverAgentTypes() { //@
        var methods []string
        for _, m := range a.Methods() {
            methods = append(methods, m.Name())
        }
        agents = append(agents,
            a.Name()+": "+strings.Join(methods, ", ")) //@
    }
    for _, t := range reflection.DiscoverTools() { //@
        tools = append(tools, t.Name())
    }
    return PlanResult{Facts: facts, Agents: agents, Tools: tools}
}))
