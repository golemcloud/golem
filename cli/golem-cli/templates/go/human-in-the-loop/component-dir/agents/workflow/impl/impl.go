// Package impl is the IMPLEMENTATION of the workflow agent. Importing it
// registers the agent.
package impl

import (
	"crypto/rand"
	"fmt"

	"component-name/agents/human"
	"component-name/agents/workflow"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct{ workflowID string }

// Each workflow instance gets its own generated id; randomness is recorded, so
// the id survives a replay unchanged.
var agent = workflow.Agent.Implement(func(workflow.ID) *state { return &state{workflowID: rand.Text()} })

func init() {
	agent.Handle(workflow.Start, func(ctx *golem.Context[state], in workflow.StartIn) string {
		// 1. Create a promise that represents waiting for human input.
		approval := golem.NewPromise[string]()

		// 2. Register the pending approval with the human over agent RPC.
		//    Normally you would surface this in a UI, email, etc.
		approver := human.Agent.Get(human.ID{Username: in.Approver})
		human.RequestApproval.MustCall(approver, human.RequestApprovalIn{
			WorkflowId: ctx.State.workflowID,
			PromiseId:  approval.ID(),
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
