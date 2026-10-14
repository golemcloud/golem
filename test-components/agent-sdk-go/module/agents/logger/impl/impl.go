// Package impl is the IMPLEMENTATION of the logging agent: one record per
// level, through slog, the standard log package, and the raw logging.Log.
package impl

import (
	"log"
	"log/slog"

	"agent-sdk-go/agents/logger"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/logging"
)

type state struct{}

var agent = logger.Agent.Implement(func(logger.Id) *state { return &state{} })

func init() {
	agent.Handle(logger.Emit, func(_ *golem.Context[state], in logger.EmitIn) golem.Unit {
		slog.Debug("debug " + in.Tag) // below the default Info level: not emitted
		slog.Info("info "+in.Tag, "n", 1)
		slog.Warn("warn " + in.Tag)
		slog.Error("error " + in.Tag)
		log.Print("stdlog " + in.Tag)
		logging.Log(logging.Trace, "raw", "trace "+in.Tag)
		logging.Log(logging.Critical, "raw", "critical "+in.Tag)
		return golem.Unit{}
	})
}
