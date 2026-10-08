// Package human is the DEFINITION of the human side of the loop: it collects
// pending approval requests and, when a decision is made, completes the
// workflow's promise. RequestApproval is called agent-to-agent; the other two
// methods are also exposed over HTTP so a UI can drive the human's decisions.
package human

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Username string }

type RequestApprovalIn struct {
	WorkflowId string
	PromiseId  golem.PromiseID
}

type DecideApprovalIn struct {
	WorkflowId string
	Decision   string
}

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:        "HumanAgent",
	Description: "Collects approval requests and completes them with a decision",
	HTTP:        &golem.Mount{Path: "/humans/{username}"},
})

var (
	RequestApproval = Agent.Method[RequestApprovalIn, string]("requestApproval",
		golem.Desc("Register a workflow waiting for this user's decision"))
	ListPendingApprovals = Agent.Method[golem.Unit, []string]("listPendingApprovals",
		golem.Desc("List the workflows waiting for a decision"),
		golem.HTTP(golem.GET("/pending")))
	DecideApproval = Agent.Method[DecideApprovalIn, string]("decideApproval",
		golem.Desc("Approve or reject a pending workflow"),
		golem.HTTP(golem.POST("/decisions")))
)
