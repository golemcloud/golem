type AccountID struct{ Team string }
type DescribeIn struct{ Email string }

var Account = golem.DefineAgent[AccountID](
    golem.Spec{Name: "AccountAgent"})
var Describe = Account.Method[DescribeIn, string]("describe",
    // over MCP, this method becomes a tool with this description:
    golem.Description("Summarize a customer account by email")) //@
var _ = Account.Implement(func(id AccountID) *AccountID {
    return &id
}).Handle(Describe, golem.Adapt((*AccountID).describe))

func (*AccountID) describe(in DescribeIn) string {
    // a client generated from the MCP server's schemas:
    // Golem checks, records and replays it like any tool call:
    // a retry resends the same idempotency key to the server:
    call := golem.Must(crm.Root.Call(func(a *crm.RootArgs) { //@
        a.Email = in.Email //@
    }))
    out := call.Collect()
    golem.Must0(out.Err)
    // the MCP server's output schema types this result:
    c := out.Result.Structured //@
    return c.Name + " is on the " + c.Plan + " plan"
}
