// Package probe is a tool whose command returns without reading its standard
// input, so a caller can check what becomes of the stdin it sent.
package probe

import (
	"io"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/tool"
)

type StdinProbe struct{}

var Tool = tool.DefineTool[StdinProbe]("stdin-probe", tool.Spec{Version: "1.0.0"})

type IgnoreArgs struct {
	Stdin io.Reader
}

var Ignore = Tool.Command[IgnoreArgs, golem.Unit]("ignore", func(a *IgnoreArgs, s *tool.CommandSpec) {
	s.Stdin(&a.Stdin).Optional()
})

var _ = Ignore.Handle(func(_ *tool.Context, _ IgnoreArgs) (golem.Unit, error) {
	return golem.Unit{}, nil
})
