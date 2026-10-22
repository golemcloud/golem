type AskIn struct {
    Question string
    Caller   golem.Principal
}
type OpenIn struct {
    N      int32
    Caller golem.Principal
}

var PrivateCharts = golem.DefineAgent[TeamID](golem.Spec{
    Name: "PrivateChartsAgent",
    // Golem checks sign-in before the HTTP call reaches the agent:
    HTTP: &golem.Mount{Path: "/private/{team}", Auth: true}}) //@
var Ask = PrivateCharts.Method[AskIn, int32]("ask",
    golem.HTTP(golem.POST("/ask")))
var Open = PrivateCharts.Method[OpenIn, *Png]("open",
    golem.HTTP(golem.GET("/charts/{n}")))
var impl = PrivateCharts.Implement(
    func(TeamID) *book { return &book{make(map[int32]Chart)} })
var _ = impl.Handle(Ask, golem.Adapt(func(b *book, a AskIn) int32 {
    // the agent charts a signed-in user's question for that user:
    png := answerWithChart(a.Question)
    b.Charts[int32(len(b.Charts)+1)] = Chart{userKey(a.Caller), png} //@
    return int32(len(b.Charts))
}))
var _ = impl.Handle(Open, golem.Adapt(func(b *book, o OpenIn) *Png {
    if made := b.Charts[o.N]; made.AskedBy == userKey(o.Caller) { //@
        return &Png{Data: made.PNG, MimeType: "image/png"}
    }
    return nil
}))
