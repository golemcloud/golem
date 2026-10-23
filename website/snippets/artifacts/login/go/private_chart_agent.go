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
    HTTP: &golem.Mount{Path: "/private/{team}", Auth: true}})
var Ask = PrivateCharts.Method[AskIn, int32]("ask",
    golem.HTTP(golem.POST("/ask")))
var Open = PrivateCharts.Method[OpenIn, *Png]("open",
    golem.HTTP(golem.GET("/charts/{n}")))
var impl = PrivateCharts.Implement(newBook)
var _ = impl.Handle(Ask, golem.Adapt(func(b *book, a AskIn) int32 {
    // the agent charts a signed-in user's question for that user:
    png := answerWithChart(a.Question)
    b.Charts[int32(len(b.Charts)+1)] = Chart{userKey(a.Caller), png} //@
    return int32(len(b.Charts))
}))
var _ = impl.Handle(Open, golem.Adapt(func(b *book, o OpenIn) *Png {
    made := b.Charts[o.N]
    // anyone else, even signed in, gets a 404:
    if made.AskedBy != userKey(o.Caller) { //@
        return nil
    }
    return &Png{Data: made.PNG, MimeType: "image/png"}
}))
