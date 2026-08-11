// Package impl is the IMPLEMENTATION of the composite-types agent.
package impl

import (
	"fmt"
	"strings"

	"agent-sdk-go/agents/richtypes"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var agent = golem.Implement(richtypes.Agent, func(richtypes.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, richtypes.Describe, func(_ *golem.Context[state], in richtypes.DescribeIn) string {
		note := "none"
		if in.Note != nil {
			note = *in.Note
		}
		return fmt.Sprintf("tags=%s note=%s", strings.Join(in.Tags, ","), note)
	})
	golem.Handle(agent, richtypes.Repeat, func(_ *golem.Context[state], in richtypes.RepeatIn) []string {
		out := make([]string, 0, in.N)
		for i := int64(0); i < in.N; i++ {
			out = append(out, in.S)
		}
		return out
	})
}
