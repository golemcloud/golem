// Package impl is the IMPLEMENTATION of the human agent. Importing it registers
// the agent.
package impl

import (
	"fmt"
	"maps"
	"slices"

	"component-name/agents/human"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct {
	username string
	pending  map[string]golem.PromiseID
}

var agent = human.Agent.Implement(func(id human.ID) *state {
	return &state{username: id.Username, pending: map[string]golem.PromiseID{}}
})

func init() {
	agent.Handle(human.RequestApproval, func(ctx *golem.Context[state], in human.RequestApprovalIn) string {
		ctx.State.pending[in.WorkflowID] = in.PromiseID
		return fmt.Sprintf("User %s received approval request for workflow %s", ctx.State.username, in.WorkflowID)
	})
	agent.Handle(human.ListPendingApprovals, func(ctx *golem.Context[state], _ golem.Unit) []string {
		return slices.Sorted(maps.Keys(ctx.State.pending))
	})
	agent.Handle(human.DecideApproval, func(ctx *golem.Context[state], in human.DecideApprovalIn) string {
		if in.Decision != "approved" && in.Decision != "rejected" {
			return fmt.Sprintf("Received invalid approval decision %s", in.Decision)
		}
		promiseID, ok := ctx.State.pending[in.WorkflowID]
		if !ok {
			return fmt.Sprintf("No pending request found for workflow %s", in.WorkflowID)
		}
		golem.CompletePromise(promiseID, in.Decision)
		delete(ctx.State.pending, in.WorkflowID)
		return fmt.Sprintf("Workflow %s was %s by %s", in.WorkflowID, in.Decision, ctx.State.username)
	})
}
