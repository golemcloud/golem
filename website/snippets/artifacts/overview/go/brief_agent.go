type BriefID struct{ Client string }
type WriteIn struct{ Notes string }

var Brief = golem.DefineAgent[BriefID](golem.Spec{
    Name: "BriefAgent",
    HTTP: &golem.Mount{Path: "/briefs/{client}",
        // Golem serves this agent's own file at /briefs/{client}:
        ExposeFiles: []golem.FileMapping{
            {Route: "/", Path: "/out/index.html"}}}, //@
})
var Write = Brief.Method[WriteIn, golem.Unit]("write")
var _ = Brief.Implement(func(id BriefID) *BriefID {
    return &id
}).Handle(Write, golem.AdaptUnit(func(b *BriefID, in WriteIn) {
    res := golem.Must(ai.Responses.New(ctx,
        responses.ResponseNewParams{Model: "gpt-4.1",
            Input: responses.ResponseNewParamsInputUnion{
                OfString: openai.String("Write an HTML brief for " +
                    b.Client + ":\n" + in.Notes)}}))
    golem.Must0(os.MkdirAll("/out", 0o755))
    // it writes the page into its own files:
    // a read waits for this call, so it never gets half a page:
    golem.Must0(os.WriteFile("/out/index.html", //@
        []byte(res.OutputText()), 0o644))
}))
