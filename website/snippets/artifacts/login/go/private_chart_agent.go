type AskIn struct{ Question string }
type OpenIn struct{ N int32 }

var PrivateCharts = golem.DefineAgent[TeamID](golem.Spec{
    Name: "PrivateChartsAgent",
    HTTP: &golem.Mount{Path: "/private/{team}", Auth: true}})
var Ask = PrivateCharts.Method[AskIn, int32]("ask",
    golem.HTTP(golem.POST("/ask")))
var Open = PrivateCharts.Method[OpenIn, *Png]("open",
    golem.HTTP(golem.GET("/charts/{n}")))
var impl = PrivateCharts.Implement(newBook)
var _ = impl.Handle(Ask, func(
    ctx *golem.Context[book], a AskIn,
) int32 {
    // the agent charts a signed-in user's question for that user:
    png := answerWithChart(a.Question)
    who, _ := ctx.Caller().Original.Ref()
    n := int32(len(ctx.State.Charts) + 1)
    ctx.State.Charts[n] = Chart{who, png} //@
    return n
})
var _ = impl.Handle(Open, func(
    ctx *golem.Context[book], o OpenIn,
) *Png {
    made := ctx.State.Charts[o.N]
    // anyone else, even signed in, gets a 404:
    if !ctx.Caller().Original.EqualsRef(made.AskedBy) { //@
        return nil
    }
    return &Png{Data: made.PNG, MimeType: "image/png"}
})
