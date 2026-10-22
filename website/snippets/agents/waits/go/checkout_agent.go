type CheckoutID struct{ OrderID string }
type PayIn struct{ Amount int32 }

var Checkout = golem.DefineAgent[CheckoutID](golem.Spec{
    Name: "CheckoutAgent",
    // webhooks require this agent to have an HTTP mount:
    HTTP: &golem.Mount{Path: "/checkout/{orderId}"},
})
var Pay = Checkout.Method[PayIn, golem.Unit]("pay")
var _ = Checkout.Implement(func(id CheckoutID) *CheckoutID {
    return &id
}).Handle(Pay, golem.AdaptUnit(func(c *CheckoutID, in PayIn) {
    paid := golem.NewWebhook[PaymentEvent]()
    createPaymentLink(c.OrderID, in.Amount, paid.URL())
    // schedule a call on another agent for tomorrow:
    mailer := Mailer.Get(MailerID{OrderID: c.OrderID})
    reminder := Remind.MustSchedule(mailer, inDays(1), golem.Unit{}) //@
    // it waits here for days if needed, using no CPU or memory:
    event := paid.MustAwait() //@
    reminder.Cancel()
    if event.Status != "succeeded" {
        return
    }
    // sleep for a week; its deadline survives restarts:
    time.Sleep(7 * 24 * time.Hour) //@
    sendSurvey(c.OrderID)
}))
