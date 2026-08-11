// Package richtypesagentimpl is the IMPLEMENTATION of the composite-types agent.
package richtypesagentimpl

import (
	"fmt"
	"strings"

	"agent-sdk-go/richtypesagent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var rich = golem.Implement(richtypesagent.Agent, func(richtypesagent.Id) *state { return &state{} })

func init() {
	golem.Handle(rich, richtypesagent.Describe, func(_ *golem.Context[state], in richtypesagent.DescribeIn) string {
		note := "none"
		if in.Note != nil {
			note = *in.Note
		}
		return fmt.Sprintf("tags=%s note=%s", strings.Join(in.Tags, ","), note)
	})
	golem.Handle(rich, richtypesagent.Repeat, func(_ *golem.Context[state], in richtypesagent.RepeatIn) []string {
		out := make([]string, 0, in.N)
		for i := int64(0); i < in.N; i++ {
			out = append(out, in.S)
		}
		return out
	})
}
