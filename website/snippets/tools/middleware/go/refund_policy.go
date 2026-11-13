// once configured, this wraps every refund call:
// agent code cannot list, remove or bypass it:
var Policy = refund.Tool.Middleware[golem.Unit]("refund-policy", //@
    tool.MiddlewareSpec{})
var _ = Policy.Handle(refund.Root, func(
    ctx *tool.MiddlewareContext[golem.Unit], a refund.RootArgs,
) (r refund.Refund, err error) {
    // reject calls by their arguments and enforce a rate limit:
    if a.Amount > 100_000 { //@
        return r, tool.ConstraintViolation("over the limit") //@
    }
    if err = checkRateLimit(ctx.Principal()); err != nil { //@
        return r, tool.ConstraintViolation("%v", err) //@
    }
    if a.Amount > 10_000 {
        // the durable caller can wait for a person across restarts:
        approval := golem.NewPromise[Verdict]() //@
        askFinance(approval.ID(), a.Charge, a.Amount)
        if !approval.MustAwait().Approved { //@
            return r, tool.ConstraintViolation("declined") //@
        }
    }
    // forwarding cannot widen the agent's permissions:
    r, err = Policy.Underlying(ctx, refund.Root).Forward(a)
    if err == nil {
        // record the refund and hide the customer's email:
        audit(a.Charge, a.Amount, r.ID) //@
        r.CustomerEmail = "[redacted]" //@
    }
    return r, err
})
