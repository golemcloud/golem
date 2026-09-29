---
name: golem-define-tool-go
description: "Defines and implements a typed Golem tool in Go. Use when creating a tool provider, a command tree with positionals, options, flags and globals, declared tool errors, or stdin/stdout-streaming commands in a Go Golem project."
---

# Define a Golem Tool in Go

A **tool** is a CLI-shaped callable unit: a command tree whose commands take arguments and return a result. It is declared from package-level vars, like an agent: the tool, its error cases and its commands. A command's arguments are an ordinary Go struct, and its spec binds each field to the command line once.

## Definition

```go
package greeter

import "github.com/golemcloud/golem/sdks/go/golem"

var Tool = golem.DefineTool("greeter", golem.ToolSpec{
	Version: "1.0.0",
	Summary: "Greets people",
})

// A declared failure: part of the command's contract, with a typed payload.
type NotFound struct{ Name string }

var ErrNotFound = golem.DefineToolError[NotFound](Tool, "not-found", golem.ToolErrorSpec{
	Kind: golem.UsageError, ExitCode: 2, Summary: "no such person",
})

type GreetArgs struct {
	Name  string
	Loud  bool
	Times int32
	Title golem.Option[string]
}

var Greet = Tool.Command[GreetArgs, string]("greet", func(a *GreetArgs, s *golem.ToolCommandSpec) {
	s.Doc("Greet someone")
	s.Positional(&a.Name).ValueName("NAME").Doc("who to greet")
	s.Flag(&a.Loud).Short('l')
	s.Option(&a.Times).Short('n').Default(1)
	s.Option(&a.Title)
	s.Raises(ErrNotFound)
})
```

The wire name is the field name in kebab case (`Times` → `times`, `GitDir` → `git-dir`); `.Name("…")` overrides it. The field's type decides the shape:

| Field | Bound with | Shape |
|---|---|---|
| `T` | `Positional`, `Option` | Required, unless it has a `.Default(v)` |
| `golem.Option[T]` | `Positional`, `Option` | Optional |
| `[]T` | `Tail` | The variadic positional after the fixed ones: `.Min`, `.Max`, `.Separator("--")`, `.Verbatim()` |
| `[]T` | `List` | `--inc a --inc b`; `.Delimited(',')` takes `--inc a,b`, `.Either(',')` both |
| `map[K]V` | `Map` | `-l k=v`; `.LastKeyWins()` instead of rejecting a repeated key |
| `bool` | `Flag` | A switch; `.Negatable()` adds `--no-<name>`, `.Default(true)` |
| `uint32` | `CountFlag` | `-vvv`; `.Max(3)` |
| `io.Reader` | `Stdin` | The command's standard input; `.Optional()`, `.Mime(…)` |
| `golem.Principal` | — | Filled by the host with who invoked the command |

A tool that cannot work without a filesystem binding says so with `ToolSpec{RequiresFilesystem: true}`; that declares the need, it does not grant access.

Every exported field must be bound, or be the principal; a mistake — an unbound field, a field bound twice, a pointer into another struct — is a definition error, reported by `golem.DefinitionErrors()` and at deploy.

Other command settings: `s.Description`, `s.Example`, `s.Aliases`, `s.ResultDoc`, `s.Formatter("json", "summary")` / `s.Formatters(…)` / `s.DefaultFormatter`, and the annotations `s.ReadOnly()`, `s.Destructive()`, `s.Idempotent()`, `s.OpenWorld()`.

## Implementation

```go
var _ = Greet.Handle(func(ctx *golem.ToolContext, a GreetArgs) (string, error) {
	if a.Name == "nobody" {
		return "", ErrNotFound.New(NotFound{Name: a.Name})
	}
	greeting := "hi " + a.Name
	if a.Loud {
		greeting = strings.ToUpper(greeting)
	}
	return strings.Repeat(greeting+"\n", int(a.Times)), nil
})
```

Handlers can live in an `impl` package; blank-import it from `main.go`, the same as an agent's. A component can export tools without defining any agent.

- Return a declared case with `ErrX.New(payload)` (`golem.Unit` for none). Returning a case the command did not list in `s.Raises` fails as an invalid result, and so does any other error or a panic.
- Use `golem.Unit` as the result type for a command that returns nothing.

## Command Tree and Globals

```go
type StockGlobals struct{ Warehouse string }

var Stock = Tool.Group("stock").Doc("Query stock levels").
	Globals[StockGlobals](func(g *StockGlobals, s *golem.ToolGlobalsSpec) {
		s.Option(&g.Warehouse).Short('w').Default("main")
	})

type ShowArgs struct {
	StockGlobals // every command below a node with globals embeds them
	Item    string
	JSON    bool
	YAML    bool
}

var Show = Stock.Command[ShowArgs, string]("show", func(a *ShowArgs, s *golem.ToolCommandSpec) {
	s.Aliases("get")
	item := s.Positional(&a.Item)
	json, yaml := s.Flag(&a.JSON), s.Flag(&a.YAML)
	s.Mutex(json, yaml)
	s.Implies(json, s.Present(&a.Warehouse))
	s.Forbids(item.ValueIs("secret"), yaml)
})
```

Globals are options and flags only; each command embeds the globals of every node on its path, the tool's own included. `Tool.Body[Args, Out](spec)` declares what the tool (or a group) does when invoked without a subcommand.

Constraints refer to bindings, `binding.ValueIs(v)` or, for any field including an inherited global, `s.Present(&a.F)` / `s.ValueIs(&a.F, v)`: `RequiresAll`, `RequiresAny`, `AllOrNone`, `Mutex`, `MutexGroups(s.AllOf(…), …)`, `Implies(lhs, rhs)` and `Forbids(lhs, …)`, with `s.AllOf`/`s.AnyOf` to group references.

## Stdin and Stdout

A command that writes standard output is declared with `StdoutCommand`; its handler gets a `*golem.ToolStdoutContext`:

```go
type UpperArgs struct{ In io.Reader }

var Upper = Tool.StdoutCommand[UpperArgs, golem.Unit]("upper", func(a *UpperArgs, s *golem.ToolCommandSpec) {
	s.Stdin(&a.In).Mime("text/plain")
	s.StdoutMime("text/plain")
})

var _ = Upper.Handle(func(ctx *golem.ToolStdoutContext, a UpperArgs) (golem.Unit, error) {
	scanner := bufio.NewScanner(a.In)
	for scanner.Scan() {
		if _, err := io.WriteString(ctx.Stdout(), strings.ToUpper(scanner.Text())+"\n"); err != nil {
			return golem.Unit{}, err
		}
	}
	return golem.Unit{}, scanner.Err()
})
```

Stdout is finished when the handler succeeds and failed when it returns an error or panics; `ctx.Stdout().Fail(golem.StreamFailed("reason"))` ends it with a specific cause. A producer failure on stdin surfaces as a `*golem.StreamError`, never as `io.EOF`.

## Deploy

Declare the tool in `golem.yaml`; agents that call it are bound to it there too (see `golem-call-tool-go`):

```yaml
tools:
  greeter: {}
```

```shell
golem build
golem deploy --yes
```

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-call-tool-go` | Calling a tool from a Go agent |
| `golem-tools-middleware-go` | Wrapping tool invocations with middleware |
| `golem-edit-manifest` | Tool declarations and bindings in `golem.yaml` |
