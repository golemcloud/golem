type WriterID struct{ Doc string }
type DraftIn struct{ Brief string }

var Writer = golem.DefineAgent[WriterID](golem.Spec{
    Name: "WriterAgent", HTTP: &golem.Mount{Path: "/docs/{doc}"},
})
var Draft = Writer.Method[DraftIn, golem.AgentStream[string]]( //@
    // clients resume this Durable Stream from an offset:
    "draft", golem.HTTP(golem.POST("/draft")))
var _ = Writer.Implement(func(id WriterID) *WriterID {
    return &id
}).Handle(Draft, golem.Adapt((*WriterID).draft))

func (*WriterID) draft(in DraftIn) golem.AgentStream[string] {
    return golem.ProduceStream(in.write)
}
func (in DraftIn) write(w *golem.AgentStreamWriter[string]) error {
    sections := []string{"Summary", "Details", "Actions"}
    for _, section := range sections {
        // completed model calls are not repeated on recovery:
        reply := golem.Must(ai.Responses.New(ctx,
            responses.ResponseNewParams{Model: "gpt-4.1",
                Input: responses.ResponseNewParamsInputUnion{
                    OfString: openai.String(in.Brief +
                        "\nWrite the " + section + " section.")}}))
        if err := w.Write(reply.OutputText()); err != nil { //@
            return err
        }
    }
    return nil
}
