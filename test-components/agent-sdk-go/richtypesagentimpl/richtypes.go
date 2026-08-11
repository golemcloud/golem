// Package richtypesagentimpl is the IMPLEMENTATION of the composite-types agent.
// Importing it registers the agent.
package richtypesagentimpl

import (
	"fmt"
	"strings"

	"agent-sdk-go/richtypesagent"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{}

var _ = golem.Implement(richtypesagent.Agent,
	func(richtypesagent.Id) *state { return &state{} },
	golem.Bound(richtypesagent.Describe, func(_ *golem.Context[state], in richtypesagent.DescribeIn) string {
		note := "none"
		if in.Note != nil {
			note = *in.Note
		}
		return fmt.Sprintf("tags=%s note=%s", strings.Join(in.Tags, ","), note)
	}),
	golem.Bound(richtypesagent.Repeat, func(_ *golem.Context[state], in richtypesagent.RepeatIn) []string {
		out := make([]string, 0, in.N)
		for i := int64(0); i < in.N; i++ {
			out = append(out, in.S)
		}
		return out
	}),
)
