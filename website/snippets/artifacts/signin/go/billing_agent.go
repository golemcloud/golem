type OrgID struct{ Org string }

var Billing = golem.DefineAgent[OrgID](golem.Spec{
    Name: "BillingAgent",
    // Golem checks the user's sign-in before any code here runs:
    HTTP: &golem.Mount{Path: "/billing/{org}", Auth: true}}) //@

type AskIn struct {
    Question string
    // Golem passes in the user who signed in:
    Caller golem.Principal //@
}

var Ask = Billing.Method[AskIn, string]("ask",
    golem.HTTP(golem.POST("/ask")))
var _ = Billing.Implement(func(id OrgID) *OrgID {
    return &id
}).Handle(Ask, golem.Adapt((*OrgID).ask))

func (b *OrgID) ask(in AskIn) string {
    user, ok := in.Caller.(golem.OidcPrincipal)
    if !ok {
        panic("sign-in required")
    }
    // the issuer can be Google, GitLab or any OIDC provider:
    issuer, sub := user.Issuer, user.Sub //@
    // the model makes a page from that user's own invoices:
    invoices := invoicesOf(b.Org, issuer, sub)
    return pageFor(in.Question, invoices)
}
