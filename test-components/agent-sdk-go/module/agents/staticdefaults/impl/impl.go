package impl

import (
	"encoding/json"

	"agent-sdk-go/agents/staticdefaults"
	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ initial string }

var agent = staticdefaults.Agent.ImplementConfigured(func(ic *golem.InitContext[staticdefaults.ID, state, staticdefaults.Config]) *state {
	return &state{initial: render(ic.Config())}
})

func init() {
	agent.Handle(staticdefaults.Read, func(ctx *golem.Context[state], _ golem.Unit) string {
		return render(ctx.Config(staticdefaults.Agent))
	})
	agent.Handle(staticdefaults.ConstructorRead, func(ctx *golem.Context[state], _ golem.Unit) string {
		return ctx.State.initial
	})
}

// Match the reference guest's diagnostic JSON, not the schema-native wire form
// or canonical config JSON (whose union is an untagged string).
func render(cfg staticdefaults.Config) string {
	var choice any
	switch value := cfg.Choice.(type) {
	case staticdefaults.Text:
		choice = map[string]string{"Text": value.Value}
	case staticdefaults.Other:
		choice = map[string]string{"Other": value.Value}
	default:
		panic("unexpected choice branch")
	}
	return string(golem.Must(json.Marshal(map[string]any{
		"settings": map[string]any{"label": cfg.Settings.Label, "count": cfg.Settings.Count},
		"choice":   choice,
		"slots":    []any{cfg.Slots.A, cfg.Slots.B, cfg.Slots.C},
		"omitted":  cfg.Omitted,
	})))
}
