// Package user is the Go tool consumer driven by go_tools.rs: it calls the
// provider's tool through the generated guest tool client.
package user

import (
	"errors"
	"fmt"
	"strings"

	vcs "golem.local/bridge/vcs-tool-guest-client"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type ID struct{ Name string }

var Agent = golem.DefineAgent[ID](golem.Spec{Name: "VcsUser"})

var Run = Agent.Method[golem.Unit, string]("run")

type state struct{}

func check(ok bool, format string, args ...any) {
	if !ok {
		panic(fmt.Sprintf(format, args...))
	}
}

func init() {
	agent := Agent.Implement(func(ID) *state { return &state{} })
	agent.Handle(Run, func(_ *golem.Context[state], _ golem.Unit) string {
		res, err := vcs.Commit.Call(func(a *vcs.CommitArgs) {
			a.Message = "fix"
			a.Paths = []string{"a.go", "b.go"}
			a.Author = golem.Some("ada")
		})
		check(err == nil, "commit: %v", err)
		check(res.Files == 2, "files: %d", res.Files)

		_, err = vcs.Commit.Call(func(a *vcs.CommitArgs) { a.Message = "secret"; a.Paths = []string{"x"} })
		var policy *golem.ToolCallError
		check(errors.As(err, &policy) && strings.Contains(policy.Message, "forbidden by policy"),
			"the policy middleware let a forbidden commit through: %v", err)

		_, err = vcs.Commit.Call(func(a *vcs.CommitArgs) { a.Message = "empty" })
		_, nothing := vcs.ErrNothingToCommit.Match(err)
		check(nothing, "an empty commit gave %v", err)

		inv, err := vcs.RemotePush.Call(func(a *vcs.RemotePushArgs) {
			a.Name = "origin"
			a.Stdin = strings.NewReader("hello tools")
		})
		check(err == nil, "push: %v", err)
		out, n, err := inv.Collect()
		check(err == nil, "push output: %v", err)
		check(string(out) == "HELLO TOOLS" && n == 11, "push gave %q and %d", out, n)

		inv, err = vcs.RemotePush.Call(func(a *vcs.RemotePushArgs) { a.Name = "forbidden" })
		check(err == nil, "push: %v", err)
		_, err = inv.Wait()
		rejected, ok := vcs.ErrRejected.Match(err)
		check(ok, "a forbidden push gave %v", err)

		return fmt.Sprintf("ok:%s:%s", res.Summary, rejected.Reason)
	})
}
