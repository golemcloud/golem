// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package tool

import (
	"testing"

	"github.com/golemcloud/golem/sdks/go/golem"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

type ExitArgs struct{ Code uint8 }

// statusProbe is the identity type of a tool whose command reports a status.
type statusProbe struct{}

// A command that returns golem.ProcessExitStatus publishes its result as a
// reference to the shared definition, which is what the Bash tool looks for,
// and its caller reads the status back.
func TestACommandResultIsTheSharedProcessExitStatus(t *testing.T) {
	r, d := newToolRegistry(), newDefinitions()
	probe := defineToolInto[statusProbe](r, d, "status-probe", Spec{Version: "1.0.0"}, false)
	exit := probe.Command[ExitArgs, golem.ProcessExitStatus]("exit", func(a *ExitArgs, s *CommandSpec) {
		s.Positional(&a.Code)
	})
	_ = exit.Handle(func(_ *Context, a ExitArgs) (golem.ProcessExitStatus, error) {
		return golem.ProcessExitStatus{ExitCode: a.Code}, nil
	})

	meta := metadataOf(t, r, d, "status-probe").wit
	var result int32 = -1
	for _, node := range meta.Commands.Nodes {
		if node.Name == "exit" && node.Body.IsSome() && node.Body.Some().Result.IsSome() {
			result = node.Body.Some().Result.Some().Type
		}
	}
	if result < 0 {
		t.Fatal("the exit command has no result")
	}
	body := meta.Schema.TypeNodes[result].Body
	if body.Tag() != types.SchemaTypeBodyRefType {
		t.Fatalf("the result has tag %d, want a ref-type", body.Tag())
	}
	if id := meta.Schema.Defs[body.RefType()].Id; id != "golem.tool.ProcessExitStatus" {
		t.Fatalf("the result refers to %q", id)
	}

	loopback(t, r, d, golem.AnonymousPrincipal{})
	status, err := exit.Call(func(a *ExitArgs) { a.Code = 3 })
	if err != nil || status.ExitCode != 3 {
		t.Fatalf("call gave %+v, %v", status, err)
	}
}
