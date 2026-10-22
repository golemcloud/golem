type SupportID struct{ Ticket string }
type SupportConfig struct {
    // the agent receives an opaque handle, not the key:
    StripeKey golem.Secret[string] //@
}
type RefundIn struct {
    Charge string
    Amount int32
}
type support struct{}

var Support = golem.DefineConfiguredAgent[SupportID, SupportConfig](
    golem.Spec{Name: "SupportAgent"})
var IssueRefund = Support.Method[RefundIn, refund.Refund](
    "issueRefund")
var _ = Support.Implement(func(SupportID) *support {
    return &support{}
}).Handle(IssueRefund, func(
    ctx *golem.Context[support], in RefundIn,
) refund.Refund {
    // refund needs a grant to reveal it; each reveal is recorded: //@
    // a binding can let a tool forward handles but reveal none: //@
    return golem.Must(refund.Root.Call(func(a *refund.RootArgs) {
        a.Charge, a.Amount = in.Charge, in.Amount
        a.APIKey = ctx.Config(Support).StripeKey //@
    }))
})
