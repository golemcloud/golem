type AnalystID struct{ Chat string }
type AskIn struct{ Question string }

var Analyst = golem.DefineAgent[AnalystID](golem.Spec{
    Name: "AnalystAgent",
    HTTP: &golem.Mount{Path: "/chats/{chat}",
        // Golem serves files this agent writes, never half-written:
        ExposeFiles: []golem.FileMapping{
            {Route: "/*", Path: "/out/$1"}}}}) //@
var Ask = Analyst.Method[AskIn, string]("ask",
    golem.HTTP(golem.POST("/ask")))
var _ = Analyst.Implement(func(id AnalystID) *AnalystID {
    return &id
}).Handle(Ask, golem.Adapt(func(a *AnalystID, in AskIn) string {
    // the model calls this tool when a user asks for a graph:
    chart := ai.NewTool("chart", //@
        "Chart sales for a quarter and link to it",
        func(_ *ai.ToolContext, args Quarter) (string, error) {
            q := args.Quarter
            golem.Must0(os.MkdirAll("/out", 0o755))
            golem.Must0(os.WriteFile("/out/"+q+".png", //@
                chartPng(q), 0o644))
            golem.Must0(os.WriteFile("/out/"+q+".html", //@
                []byte(pageFor(q)), 0o644))
            return "/chats/" + a.Chat + "/" + q + ".html", nil
        })
    return golem.Must(genkit.GenerateText(ctx, g,
        ai.WithModelName(model), ai.WithPrompt(in.Question),
        ai.WithTools(chart), ai.WithMaxTurns(5)))
}))
