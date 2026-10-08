// Package vcs is the Go tool fixture driven by go_tools.rs: a tool with
// globals, a group, a tail, a command with stdout and stderr, standard input and declared
// errors, a typed middleware over it, and an agent that calls the tool from its
// own component.
package vcs

import (
	"fmt"
	"io"
	"strings"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
)

// Vcs is the tool's identity type.
type Vcs struct{}

var Tool = tool.DefineTool[Vcs]("vcs", tool.Spec{Version: "1.0.0", Summary: "A tiny version control tool"})

type Globals struct{ Dir string }

var _ = Tool.Globals[Globals](func(g *Globals, s *tool.GlobalsSpec) {
	s.Option(&g.Dir).Short('C').Default(".")
})

var ErrNothingToCommit = tool.DefineToolError[golem.Unit](Tool, "nothing-to-commit",
	tool.ErrorSpec{Kind: tool.RuntimeError, ExitCode: 1})

type Rejected struct{ Reason string }

var ErrRejected = tool.DefineToolError[Rejected](Tool, "rejected",
	tool.ErrorSpec{Kind: tool.RuntimeError, ExitCode: 3})

type CommitArgs struct {
	Globals
	Paths    []string
	Message  string
	Author   golem.Option[string]
	Priority golem.Option[uint32]
	Amend    bool
	Caller   golem.Principal
}

type CommitResult struct {
	Summary string
	Files   int32
}

var Commit = Tool.Command[CommitArgs, CommitResult]("commit", func(a *CommitArgs, s *tool.CommandSpec) {
	s.Doc("Record changes")
	s.Tail(&a.Paths)
	s.Option(&a.Message).Short('m')
	s.Option(&a.Author)
	s.Option(&a.Priority).MaxValue(golem.Some[uint32](5))
	s.Flag(&a.Amend)
	s.Raises(ErrNothingToCommit)
})

type PushArgs struct {
	Globals
	Name string
	In   io.Reader
}

var Push = Tool.Group("remote").OutputCommand[PushArgs, int32]("push", func(a *PushArgs, s *tool.CommandSpec) {
	s.Positional(&a.Name)
	s.Stdin(&a.In).Optional()
	s.Stdout().Mime("text/plain")
	s.Stderr().Doc("progress")
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

var _ = Commit.Handle(func(_ *tool.Context, a CommitArgs) (CommitResult, error) {
	if len(a.Paths) == 0 && !a.Amend {
		return CommitResult{}, ErrNothingToCommit.New(golem.Unit{})
	}
	author := a.Author.Or("anonymous")
	return CommitResult{
		Summary: fmt.Sprintf("%s|%s|%s|%s", a.Dir, a.Message, author, describe(a.Caller)),
		Files:   int32(len(a.Paths)),
	}, nil
})

var _ = Push.Handle(func(ctx *tool.OutputContext, a PushArgs) (int32, error) {
	if _, err := io.WriteString(ctx.Stderr(), "pushing "+a.Name); err != nil {
		return 0, err
	}
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

type DelegateArgs struct {
	Globals
	Card golem.PermissionCard
}

// Delegate hands a permission card back; publishing it checks that the host
// accepts cards in tool signatures.
var Delegate = Tool.Command[DelegateArgs, golem.PermissionCard]("delegate", func(a *DelegateArgs, s *tool.CommandSpec) {
	s.Positional(&a.Card)
})

var _ = Delegate.Handle(func(_ *tool.Context, a DelegateArgs) (golem.PermissionCard, error) { return a.Card, nil })

type PolicyParams struct{ ForbidMessage string }

// Policy is a transparent middleware: it checks and rewrites commit, and the
// tool's other commands pass straight through it.
var Policy = Tool.Middleware[PolicyParams]("vcs-policy", tool.MiddlewareSpec{Version: "1.0.0"})

var _ = Policy.Handle(Commit, func(ctx *tool.MiddlewareContext[PolicyParams], a CommitArgs) (CommitResult, error) {
	if a.Message == ctx.Parameters().ForbidMessage {
		return CommitResult{}, tool.ConstraintViolation("message %q is forbidden by policy", a.Message)
	}
	a.Message = "checked:" + a.Message
	return Policy.Underlying(ctx, Commit).Forward(a)
})

type AuditParams struct{ BlockedMessage string }

// Audit is a universal middleware installed for the whole environment: it reads
// the arguments of any command as JSON.
var Audit = tool.DefineUniversalToolMiddleware[AuditParams]("vcs-audit", tool.MiddlewareSpec{Version: "1.0.0"})

var _ = Audit.Handle(func(ctx *tool.UniversalMiddlewareContext[AuditParams]) (golem.Option[golem.TypedValue], error) {
	args, err := ctx.Input().JSON()
	if err != nil {
		return golem.None[golem.TypedValue](), err
	}
	if fields, ok := args.(map[string]any); ok && fields["message"] == ctx.Parameters().BlockedMessage {
		return golem.None[golem.TypedValue](), tool.ConstraintViolation("blocked by the environment audit")
	}
	return ctx.Next(ctx.Input())
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
