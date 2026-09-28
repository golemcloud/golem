---
name: golem-call-tool-go
description: "Calls a Golem tool from a Go agent. Use when invoking a tool provider from Go through the discovered tool client, passing arguments as canonical JSON, and handling validation or tool failures."
---

# Call a Golem Tool from Go

A Go agent calls a tool through **discovery**: `golem.DiscoverTool` returns a snapshot of the deployed tool, and the client bound from it validates and packs every call against that snapshot. Arguments and results are canonical JSON, because the caller does not share the tool's Go types.

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

## Invoke a Command

```go
import (
	"errors"
	"fmt"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	"github.com/golemcloud/golem/sdks/go/golem"
)

func greet(name string) (string, error) {
	tool, found := golem.DiscoverTool("greeter")
	if !found {
		return "", fmt.Errorf("the greeter tool is not available to this agent")
	}
	client, err := tool.Bind()
	if err != nil {
		return "", err
	}
	out, err := client.InvokeAndAwait([]string{"greet"}, map[string]any{
		"name":  name,
		"loud":  false,
		"times": 2,
	})
	if err != nil {
		var invalid *core.ValidationError
		if errors.As(err, &invalid) {
			for _, issue := range invalid.Issues {
				fmt.Println(issue.Path, issue.Message)
			}
		}
		return "", err
	}
	return out.(string), nil
}
```

- The command path is the list of subcommand names from the root (aliases work); `nil` addresses the root body.
- Supply every argument by its wire name: positionals, option long names and flags. A flag is a `bool`. Nothing is filled in from declared defaults.
- The result is canonical JSON (`string`, `float64`, `map[string]any`, …), or `nil` for a command without a result.
- Arguments that do not match the snapshot fail with `*schema.ValidationError` before anything is sent. A tool failure, denial or declared tool error comes back as an ordinary `error` whose message names it.

## Inspect Before Calling

```go
tool, _ := golem.DiscoverTool("greeter")
cmd, ok := tool.Command([]string{"greet"})
if ok && cmd.Callable() {
	params := golem.Must(cmd.Arguments())   // []schema.Parameter, positionals first
	schema := golem.Must(cmd.ToJSONSchema(true))
	_, returns := cmd.Result()
	for _, e := range cmd.Errors() {
		fmt.Println(e.Name, e.ExitCode)
	}
	_, _, _ = params, schema, returns
}
```

`golem.DiscoverTools()` lists every tool the agent may reach, and `tool.Commands()` walks the whole tree. A node that only groups subcommands is discoverable but not `Callable`. A snapshot never refreshes itself; discover again when a newer deployment matters.

## Already-Packed Values

`golem.BindTool(name)` binds by name without a snapshot. Its `InvokeDynamic(path, input)` takes a `golem.TypedValue` and returns `golem.Option[golem.TypedValue]`; nothing is validated locally, so use it only to forward values that are already packed.

## Current Surface

- Calls are awaited: there is no trigger, schedule or cancellation form.
- A command's stdin and stdout streams cannot be supplied or read from a Go caller.
- A command with a repeatable list option cannot be packed: `Arguments()` reports an error for it.

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-define-tool-go` | Defining the tool being called |
| `golem-agent-reflection-go` | Discovering and calling agents the same way |
| `golem-tools-middleware-go` | Intercepting tool calls |
