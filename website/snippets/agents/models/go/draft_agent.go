var g = genkit.Init(ctx,
    genkit.WithPlugins(&openai.OpenAI{}, &anthropic.Anthropic{}))

type Review struct {
    OK    bool   `json:"ok"`
    Notes string `json:"notes"`
}
type Draft struct {
    Text   string
    Review Review
}
type DraftID struct{ Topic string }
type WriteIn struct{ Idea string }

var Drafter = golem.DefineAgent[DraftID](
    golem.Spec{Name: "DraftAgent"})
var Write = Drafter.Method[WriteIn, Draft]("write")
var _ = Drafter.Implement(func(id DraftID) *DraftID {
    return &id
}).Handle(Write, golem.Adapt(func(d *DraftID, in WriteIn) Draft {
    // draft with one provider, review with another:
    draft := golem.Must(genkit.GenerateText(ctx, g,
        ai.WithModelName("openai/gpt-4.1"), //@
        ai.WithPrompt("Product update on "+d.Topic+": "+in.Idea)))
    review, _, err := genkit.GenerateData[Review](ctx, g,
        ai.WithModelName("anthropic/claude-sonnet-4-5"), //@
        ai.WithPrompt("Review this product update:\n"+draft))
    golem.Must0(err)
    return Draft{Text: draft, Review: *review}
}))
