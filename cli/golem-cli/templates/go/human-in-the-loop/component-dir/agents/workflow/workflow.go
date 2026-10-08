// Package workflow is the DEFINITION of the workflow side of the loop: it
// creates a promise, hands it to a human for approval, then pauses until the
// promise is completed — the classic human-in-the-loop pattern.
package workflow

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

type StartIn struct{ Approver string }

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:        "WorkflowAgent",
	Description: "A workflow that pauses until a human approves or rejects it",
	HTTP:        &golem.Mount{Path: "/workflows/{name}"},
})

var Start = Agent.Method[StartIn, string]("start",
	golem.Desc("Ask the approver for a decision and wait for it"),
	golem.HTTP(golem.POST("/start")))
