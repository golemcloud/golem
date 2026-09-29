---
name: golem-call-tool-go
description: "Calls a Golem tool from a Go agent. Use when invoking a tool with typed arguments, through its own definition or a generated guest tool client, streaming its stdin/stdout, handling declared tool errors, or calling a tool discovered at runtime."
---

# Call a Golem Tool from Go

A Go agent calls a tool's command with the command's own argument struct: `Call` starts from the declared defaults, lets you set the rest, and returns the typed result.

## Grant the Tool

The calling agent must be bound to the tool in `golem.yaml`:

```yaml
tools:
  greeter: {}

agents:
  AssistantAgent:
    tools:
      greeter: {}
```

A binding on a component applies to all of its agents (see `golem-edit-manifest`).

## A Tool of the Same Component

Call the command declared with `golem.DefineTool` (see `golem-define-tool-go`):

```go
greeting, err := greeter.Greet.Call(func(a *greeter.GreetArgs) {
	a.Name = "ada"
	a.Loud = true
})
if nf, ok := greeter.ErrNotFound.Match(err); ok {
	return "no such person: " + nf.Name
}
if err != nil {
	return "", err
}
```

`fill` may be `nil` when the defaults are enough. The call goes through the runtime like any other, so the tool's middleware applies, and the handler's principal is the one the calling agent's invocation runs on behalf of.

## A Tool of Another Component

Depend on the tool, and `golem build` generates a typed client for it:

```yaml
components:
  example:caller:
    dir: caller
    templates: go
    dependencies:
      tools:
        - example:provider/greeter
```

The client is `golem-temp/bridge-sdk/go/internal/greeter-tool-guest-client`, a Go module named `golem.local/bridge/greeter-tool-guest-client`. Point the calling component's `go.mod` at it:

```
require golem.local/bridge/greeter-tool-guest-client v0.0.0

replace golem.local/bridge/greeter-tool-guest-client => ../golem-temp/bridge-sdk/go/internal/greeter-tool-guest-client
```

It declares the tool with `golem.DeclareRemoteTool` and each command the same way, so calls look identical:

```go
import greeter "golem.local/bridge/greeter-tool-guest-client"

greeting, err := greeter.Greet.Call(func(a *greeter.GreetArgs) { a.Name = "ada" })
```

Names follow the command path: `Tool`, `Root` for the tool's own body, `Greet`, `StockShow` for `stock show`, with `<Command>Args` structs, `<Node>Globals` structs embedded in them, and `Err<Case>` error cases (`Err<Command><Case>` when two commands give one case different payloads). Defaults a Go literal can spell are filled in; set any other field yourself.

## Stdin and Stdout

A field bound with `Stdin` is an `io.Reader`; set it to feed the command. A required one left nil is refused before anything is sent. A command declared with `StdoutCommand` returns a running invocation instead of the result:

```go
inv, err := tools.Upper.Call(func(a *tools.UpperArgs) { a.In = strings.NewReader("hello") })
if err != nil {
	return err
}
out, _, err := inv.Collect() // or read inv.Stdout(), then inv.Wait()
```

Read the output while the command runs: `Wait` alone stalls a command that writes more than the stream buffers. `inv.Cancel()` asks the runtime to cancel the call.

## Errors

A failed call returns a `*golem.ToolCallError` (`errors.As`): its `Kind` separates the tool's declared errors (`ToolCallDeclaredError`, matched with `ErrX.Match`) from rejected input, constraint violations, denials, cancellation and runtime failures. `Message` carries the detail.

## Discovered Tools

A tool with no Go declaration is called through **discovery**: `golem.DiscoverTool` returns a snapshot of the deployed tool, and the client bound from it validates and packs every call against it, with canonical JSON arguments and results.

```go
tool, found := golem.DiscoverTool("greeter")
if !found {
	return fmt.Errorf("the greeter tool is not available to this agent")
}
client, err := tool.Bind()
if err != nil {
	return err
}
out, err := client.InvokeAndAwait([]string{"greet"}, map[string]any{
	"name": "ada", "loud": false, "times": 2, "title": nil,
})
```

- Supply every field of the command's input record by its wire name: inherited globals, positionals, the tail (a list), options and flags. `cmd.Arguments()` lists them in order. Nothing is filled in from declared defaults.
- Arguments that do not match the snapshot fail with `*schema.ValidationError` before anything is sent.
- `tool.Command(path)`, `tool.Commands()`, `cmd.Result()`, `cmd.Errors()` and `cmd.ToJSONSchema(true)` describe the tool; a snapshot never refreshes itself.
- `golem.BindTool(name)` binds by name without a snapshot; its `InvokeDynamic(path, input)` forwards an already packed `golem.TypedValue` unchecked.

Discovered calls are awaited, and cannot supply stdin or read stdout.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-define-tool-go` | Defining the tool being called |
| `golem-agent-reflection-go` | Discovering and calling agents the same way |
| `golem-tools-middleware-go` | Intercepting tool calls |
