---
name: golem-tools-middleware-go
description: "Defines Golem tool middleware in Go. Use for validation, policy, auditing, caching, adapting one tool to another, or rewriting tool calls and results, either typed for one tool or universal for any tool, in a Go Golem project."
---

# Tool Middleware in Go

A middleware wraps the invocations of a tool: it sees a call on its way in, hands it to the layer beneath, and sees the outcome on its way back. Prefer the **typed** form: it is declared on the tool it presents, and handles that tool's commands with their own argument and result types. Use the **universal** form only for middleware that must work with any tool, such as an audit log.

## Typed Middleware

```go
package policy

import (
	"errors"

	"myapp/tools/inventory"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// Static configuration of one installation; its schema is published.
type Params struct{ MaxAdjust int32 }

var Policy = inventory.Tool.Middleware[Params]("inventory-policy", golem.ToolMiddlewareSpec{
	Version: "1.0.0",
	Summary: "Caps stock adjustments",
})

var _ = Policy.Handle(inventory.Adjust, func(ctx *golem.ToolMiddlewareContext[Params], a inventory.AdjustArgs) (int32, error) {
	if a.By > ctx.Parameters().MaxAdjust {
		return 0, errors.New("adjustment too large")
	}
	return Policy.Underlying(ctx, inventory.Adjust).Forward(a)
})
```

- `Tool.Middleware` is **transparent**: it presents and wraps the same tool, and every command it does not `Handle` goes straight to the tool.
- The handler has a tool handler's shape: it returns the command's result, a declared error with `ErrX.New(payload)`, or any other error to fail the call. Returning an error from the layer beneath passes it on unchanged, so `ErrX.Match` still works for the caller.
- `Policy.Underlying(ctx, cmd)` reaches a command of the wrapped tool: `Forward(a)` sends complete arguments unchanged, and `Call(fill)` starts from the command's defaults. Passing a command of another tool is a compile error.
- `ctx.Parameters()`, `ctx.Principal()`, `ctx.ToolName()` and `ctx.CommandPath()` describe the invocation. Standard input travels in the args struct, so forwarding `a` forwards it.

Blank-import the package from `main.go`. Use `golem.Unit` as the parameter type for a middleware that takes none.

## Commands That Write Standard Output

```go
var _ = Policy.HandleStdout(inventory.Import, func(ctx *golem.ToolMiddlewareStdoutContext[Params], a inventory.ImportArgs) (int32, error) {
	inv, err := Policy.UnderlyingStdout(ctx, inventory.Import).Start(a)
	if err != nil {
		return 0, err
	}
	if _, err := io.Copy(ctx.Stdout(), redact(inv.Stdout())); err != nil {
		return 0, err
	}
	return inv.Wait()
})
```

`UnderlyingStdout(...).Forward(a)` copies the output through unchanged and returns the result; `Start` returns the running invocation for rewriting it. `ctx.Stdout()` is finished when the handler succeeds and failed when it returns an error or panics.

## Adapters

An adapter presents one tool over another — for example a new version of a tool's interface over the old implementation:

```go
var Compat = invv2.Tool.Adapter[golem.Unit]("inventory-v2-on-v1", invv1.Tool, golem.ToolMiddlewareSpec{Version: "1.0.0"})

var _ = Compat.Handle(invv2.Show, func(ctx *golem.ToolMiddlewareContext[golem.Unit], a invv2.ShowArgs) (string, error) {
	return Compat.Underlying(ctx, invv1.Get).Call(func(b *invv1.GetArgs) { b.Sku = a.Sku })
})
```

An adapter must handle every command of the tool it presents; a missing one is a definition error. The wrapped tool can be one this component defines, or a tool client (`golem.DefineToolClient`, see `golem-call-tool-go`).

## Universal Middleware

A universal middleware has no Go types for the tools it wraps: the arguments and the result travel as `golem.TypedValue`, which carries its own schema.

```go
type AuditParams struct {
	Channel string
	Deny    bool
}

var Audit = golem.DefineUniversalToolMiddleware[AuditParams]("audit", golem.ToolMiddlewareSpec{
	Version: "1.0.0",
	Summary: "Records every tool invocation",
})

var _ = Audit.Handle(func(ctx *golem.UniversalToolMiddlewareContext[AuditParams]) (golem.Option[golem.TypedValue], error) {
	p := ctx.Parameters()
	slog.Info("tool call", "channel", p.Channel, "tool", ctx.ToolName(), "command", ctx.CommandPath())
	if p.Deny {
		return golem.None[golem.TypedValue](), errors.New("denied by audit policy")
	}
	return ctx.Next(ctx.Input()) // forwards stdin, copies stdout into ctx.Stdout()
})
```

- The result is `golem.None` for a command without one.
- `ctx.Input().WithJSON(newArgs)` rewrites the arguments before `Next`; read and build values with `v.JSON()`, `golem.DecodeTypedValue[T]` and `golem.EncodeTypedValue`.
- `ctx.Start(input)` returns the running invocation, for rewriting standard output.
- A failure beneath is a `*golem.ToolCallError`; return it to pass it on unchanged.
- `ctx.ToolMetadata()` describes the wrapped tool (commands, arguments, results and errors), as in `golem-call-tool-go`.

## Install

Declare the middleware, then install it on a tool binding or for a whole environment:

```yaml
tools:
  inventory: {}
  middleware:
    inventory-policy:
      component: my-app:policy
    audit:
      component: my-app:policy

agents:
  AssistantAgent:
    tools:
      inventory:
        middleware:
          - name: inventory-policy
            parameters: { maxAdjust: 100 }

environments:
  local:
    tools:
      middleware: [audit]
```

Parameter keys are the canonical field names (`MaxAdjust` → `maxAdjust`).

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-define-tool-go` | Defining the tools a middleware wraps |
| `golem-call-tool-go` | Calling a tool through its middleware chain |
| `golem-edit-manifest` | Tool bindings and middleware installation |
