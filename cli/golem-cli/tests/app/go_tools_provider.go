// Package vcs is the Go tool fixture driven by go_tools.rs: a tool with
// globals, a group, a tail, a stdout command, standard input and declared
// errors, a typed middleware over it, and an agent that calls the tool from its
// own component.
package vcs

import (
	"fmt"
	"io"
	"strings"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// Vcs is the tool's identity type.
type Vcs struct{}

var Tool = golem.DefineTool[Vcs]("vcs", golem.ToolSpec{Version: "1.0.0", Summary: "A tiny version control tool"})

type Globals struct{ Dir string }

var _ = Tool.Globals[Globals](func(g *Globals, s *golem.ToolGlobalsSpec) {
	s.Option(&g.Dir).Short('C').Default(".")
})

var ErrNothingToCommit = golem.DefineToolError[golem.Unit](Tool, "nothing-to-commit",
	golem.ToolErrorSpec{Kind: golem.RuntimeError, ExitCode: 1})

type Rejected struct{ Reason string }

var ErrRejected = golem.DefineToolError[Rejected](Tool, "rejected",
	golem.ToolErrorSpec{Kind: golem.RuntimeError, ExitCode: 3})

type CommitArgs struct {
	Globals
	Paths   []string
	Message string
	Author  golem.Option[string]
	Amend   bool
	Caller  golem.Principal
}

type CommitResult struct {
	Summary string
	Files   int32
}

var Commit = Tool.Command[CommitArgs, CommitResult]("commit", func(a *CommitArgs, s *golem.ToolCommandSpec) {
	s.Doc("Record changes")
	s.Tail(&a.Paths)
	s.Option(&a.Message).Short('m')
	s.Option(&a.Author)
	s.Flag(&a.Amend)
	s.Raises(ErrNothingToCommit)
})

type PushArgs struct {
	Globals
	Name string
	In   io.Reader
}

var Push = Tool.Group("remote").StdoutCommand[PushArgs, int32]("push", func(a *PushArgs, s *golem.ToolCommandSpec) {
	s.Positional(&a.Name)
	s.Stdin(&a.In).Optional()
	s.Raises(ErrRejected)
})

func describe(p golem.Principal) string {
	switch p.(type) {
	case golem.AgentPrincipal:
		return "agent"
	case golem.GolemUserPrincipal:
		return "golem-user"
	default:
		return fmt.Sprintf("%T", p)
	}
}

var _ = Commit.Handle(func(_ *golem.ToolContext, a CommitArgs) (CommitResult, error) {
	if len(a.Paths) == 0 && !a.Amend {
		return CommitResult{}, ErrNothingToCommit.New(golem.Unit{})
	}
	author := a.Author.Or("anonymous")
	return CommitResult{
		Summary: fmt.Sprintf("%s|%s|%s|%s", a.Dir, a.Message, author, describe(a.Caller)),
		Files:   int32(len(a.Paths)),
	}, nil
})

var _ = Push.Handle(func(ctx *golem.ToolStdoutContext, a PushArgs) (int32, error) {
	if a.Name == "forbidden" {
		return 0, ErrRejected.New(Rejected{Reason: "protected " + a.Name})
	}
	if a.In == nil {
		return 0, nil
	}
	data, err := io.ReadAll(a.In)
	if err != nil {
		return 0, err
	}
	n, err := io.WriteString(ctx.Stdout(), strings.ToUpper(string(data)))
	return int32(n), err
})

type PolicyParams struct{ Forbid string }

// Policy is a transparent middleware: it checks and rewrites commit, and the
// tool's other commands pass straight through it.
var Policy = Tool.Middleware[PolicyParams]("vcs-policy", golem.ToolMiddlewareSpec{Version: "1.0.0"})

var _ = Policy.Handle(Commit, func(ctx *golem.ToolMiddlewareContext[PolicyParams], a CommitArgs) (CommitResult, error) {
	if a.Message == ctx.Parameters().Forbid {
		return CommitResult{}, fmt.Errorf("message %q is forbidden by policy", a.Message)
	}
	a.Message = "checked:" + a.Message
	return Policy.Underlying(ctx, Commit).Forward(a)
})

type SelfID struct{ Name string }

var Self = golem.DefineAgent[SelfID](golem.Spec{Name: "VcsSelfCaller"})

// Run calls the component's own tool with typed arguments.
var Run = Self.Method[golem.Unit, string]("run")

type selfState struct{}

func init() {
	self := Self.Implement(func(SelfID) *selfState { return &selfState{} })
	self.Handle(Run, func(_ *golem.Context[selfState], _ golem.Unit) string {
		res, err := Commit.Call(func(a *CommitArgs) {
			a.Message = "self"
			a.Paths = []string{"x"}
		})
		if err != nil {
			return "err:" + err.Error()
		}
		return fmt.Sprintf("ok:%s:%d", res.Summary, res.Files)
	})
}
