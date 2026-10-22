type Args struct {
    Charge string
    Amount int32
    // apiKey is an opaque handle; revealing it takes a grant:
    APIKey golem.Secret[string]
}

var Tool = tool.DefineTool[RefundTool]("refund",
    tool.Spec{Version: "1.0.0"})
var ErrHTTP = tool.DefineToolError[golem.Unit](Tool, "http",
    tool.ErrorSpec{Kind: tool.RuntimeError, ExitCode: 1})
var _ = Tool.Body[Args, Refund](
    func(a *Args, s *tool.CommandSpec) {
        s.Positional(&a.Charge)
        s.Positional(&a.Amount)
        s.Positional(&a.APIKey)
        s.Raises(ErrHTTP)
    }).Handle(refund)

// each call gets a fresh VM that can't read the agent's memory:
func refund(_ *tool.Context, a Args) (out Refund, err error) { //@
    key := a.APIKey.MustGet()
    // on replay, a completed request returns its recorded response:
    res := golem.Must(StripeRefund(key, a.Charge, a.Amount)) //@
    defer res.Body.Close()
    if res.StatusCode/100 != 2 {
        return Refund{}, ErrHTTP.New(golem.Unit{})
    }
    err = json.NewDecoder(res.Body).Decode(&out)
    return out, err
}
