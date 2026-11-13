var Tool = tool.DefineTool[Summarizer]("summarize", tool.Spec{})
var Summarize = Tool.Body[Args, string](
    func(a *Args, s *tool.CommandSpec) { s.Positional(&a.URL) })
var _ = Summarize.Handle(summarize)

// each call gets a fresh VM that can't read the agent's memory:
func summarize(_ *tool.Context, a Args) (string, error) { //@
    // recovery reuses this completed fetch's response:
    res := golem.Must(http.Get(a.URL)) //@
    defer res.Body.Close()
    page := golem.Must(io.ReadAll(res.Body))
    return summarizeText(string(page)), nil
}

var Digest = golem.DefineAgent[DigestID](
    golem.Spec{Name: "DigestAgent"})
var Build = Digest.Method[BuildIn, []string]("build")
var _ = Digest.Implement(func(DigestID) *digest {
    return &digest{}
}).Handle(Build, golem.Adapt(func(_ *digest, in BuildIn) []string {
    var out []string
    for _, url := range in.URLs {
        out = append(out, golem.Must(Summarize.Call(func(a *Args) {
            a.URL = url
        })))
    }
    return out
}))
