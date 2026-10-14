// Package runner is the Go consumer driven by go_tools.rs: it runs a script
// through the client generated for the built-in bash tool.
package runner

import (
	"fmt"

	bash "golem.local/bridge/bash-tool-guest-client"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type ID struct{ Name string }

var Agent = golem.DefineAgent[ID](golem.Spec{Name: "ScriptRunner"})

type RunScriptIn struct{ Script string }

var RunScript = Agent.Method[RunScriptIn, string]("run-script")

type state struct{}

var agent = Agent.Implement(func(ID) *state { return &state{} })

func init() {
	agent.Handle(RunScript, func(_ *golem.Context[state], in RunScriptIn) string {
		result, err := bash.Run.Call(func(a *bash.RunArgs) { a.Script = in.Script })
		if err != nil {
			panic(err)
		}
		if result.ExitCode != 0 {
			panic(fmt.Sprintf("the script exited with %d: %s", result.ExitCode, result.Stderr))
		}
		return result.Stdout
	})
}
