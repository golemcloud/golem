type OrgID struct{ Org string }

var Billing = golem.DefineAgent[OrgID](golem.Spec{
    Name: "BillingAgent",
    // Golem checks the user's sign-in before any code here runs:
    HTTP: &golem.Mount{Path: "/billing/{org}", Auth: true}}) //@

type AskIn struct{ Question string }

var Ask = Billing.Method[AskIn, string]("ask",
    golem.HTTP(golem.POST("/ask")))
var _ = Billing.Implement(func(id OrgID) *OrgID {
    return &id
}).Handle(Ask, func(ctx *golem.Context[OrgID], in AskIn) string {
    // Golem passes in the user who signed in:
    user, ok := ctx.Caller().Original.(golem.UserPrincipal) //@
    if !ok {
        panic("sign-in required")
    }
    // the issuer can be Google, GitLab or any OIDC provider:
    issuer, subject := user.Pair.Issuer, user.Pair.Subject //@
    // the model makes a page from that user's own invoices:
    invoices := invoicesOf(ctx.State.Org, issuer, subject)
    return pageFor(in.Question, invoices)
})
