type SupportID struct{ Ticket string }
type chat struct{ history []Message }

var Support = golem.DefineAgent[SupportID](golem.Spec{
    Name: "SupportAgent",
    HTTP: &golem.Mount{Path: "/tickets/{ticket}"},
})
var Handle = Support.Method[Ticket, string]("handle")
var _ = Support.Implement(func(SupportID) *chat {
    return &chat{}
}).Handle(Handle, golem.Adapt(func(c *chat, in Ticket) string {
    // after a crash or restart, this history is restored:
    c.history = append(c.history, openai.UserMessage(in.Message)) //@
    reply := readReply(golem.Must(ai.Chat.Completions.New(ctx,
        openai.ChatCompletionNewParams{Model: "gpt-4.1",
            Messages: c.history, ResponseFormat: replyFormat})))
    c.history = append(c.history,
        openai.AssistantMessage(reply.Text))
    if !reply.RefundRequested {
        return reply.Text
    }
    approval := golem.NewWebhook[Approval]()
    notifyReviewer(approval.URL(), in.OrderID, reply.Amount)
    // it can wait here for days, then resume mid-function:
    approved := approval.MustAwait().Approved //@
    // it refunds once if the payment API honors idempotency keys:
    if approved {
        refundOrder(in.OrderID, reply.Amount)
    }
    return reply.Text
}))
