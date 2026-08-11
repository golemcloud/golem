// Package richtypesagent is the DEFINITION of the composite-types agent. The
// behaviour lives in richtypesagentimpl.
package richtypesagent

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

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

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "RichAgent", Description: "Composite type round-trips", Mode: golem.Durable,
})

var (
	Describe = golem.DefineMethod[Id, DescribeIn, string]("describe",
		golem.Desc("Summarize a list + optional argument"))
	Repeat = golem.DefineMethod[Id, RepeatIn, []string]("repeat",
		golem.Desc("Return a list of N copies of S"))
)
