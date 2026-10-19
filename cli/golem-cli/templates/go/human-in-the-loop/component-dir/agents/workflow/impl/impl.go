// Package impl is the IMPLEMENTATION of the workflow agent. Importing it
// registers the agent.
package impl

import (
	"fmt"

	"component-name/agents/human"
	"component-name/agents/workflow"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ workflowID string }

var agent = workflow.Agent.Implement(func(id workflow.ID) *state { return &state{workflowID: id.WorkflowID} })

func init() {
	agent.Handle(workflow.Start, func(ctx *golem.Context[state], in workflow.StartIn) string {
		// 1. Create a promise that represents waiting for human input.
		approval := golem.NewPromise[string]()

		// 2. Register the pending approval with the human over agent RPC.
		//    Normally you would surface this in a UI, email, etc.
		approver := human.Agent.Get(human.ID{Username: in.Approver})
		human.RequestApproval.MustCall(approver, human.RequestApprovalIn{
			WorkflowID: ctx.State.workflowID,
			PromiseID:  approval.ID(),
		})

		// 3. Pause here until the promise is completed by the human.
		decision := approval.MustAwait()

		// 4. Continue based on the human decision.
		if decision == "approved" {
			return fmt.Sprintf("Workflow %s was approved", ctx.State.workflowID)
		}
		return fmt.Sprintf("Workflow %s was rejected", ctx.State.workflowID)
	})
}
