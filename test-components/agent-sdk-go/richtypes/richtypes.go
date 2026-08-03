// Package richtypes exercises composite value types over the invocation wire:
// a list + optional input, and a list output. The codec is exhaustively unit-
// tested natively; this confirms it round-trips through the real host path.
package richtypes

import (
	"fmt"
	"strings"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type Id struct{ Name string }
type State struct{}

var Agent = golem.DefineAgent[Id, State](
	golem.Spec{Name: "RichAgent", Description: "Composite type round-trips", Mode: golem.Durable},
	func(Id) *State { return &State{} },
)

// DescribeIn flattens to two params: a list<string> and an option<string>.
type DescribeIn struct {
	Tags []string
	Note *string
}

// RepeatIn flattens to a string and an s64.
type RepeatIn struct {
	S string
	N int64
}

var (
	Describe = golem.DefineMethod[Id, DescribeIn, string]("describe",
		golem.Desc("Summarize a list + optional argument"))
	Repeat = golem.DefineMethod[Id, RepeatIn, []string]("repeat",
		golem.Desc("Return a list of N copies of S"))
)

func init() {
	golem.Implement(Agent, Describe, func(_ *golem.Context[State], in DescribeIn) string {
		note := "none"
		if in.Note != nil {
			note = *in.Note
		}
		return fmt.Sprintf("tags=%s note=%s", strings.Join(in.Tags, ","), note)
	})
	golem.Implement(Agent, Repeat, func(_ *golem.Context[State], in RepeatIn) []string {
		out := make([]string, 0, in.N)
		for i := int64(0); i < in.N; i++ {
			out = append(out, in.S)
		}
		return out
	})
}
