// Package impl is the IMPLEMENTATION of the promise agent. Create mints a
// promise and keeps the handle in state, returning its oplog index so an external
// completer (the executor's complete-promise API, or another agent) can address
// it; Await suspends the worker until that promise is completed.
package impl

import (
	"fmt"

	"agent-sdk-go/agents/promises"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// state keeps the created promises by oplog index, so Await can pick the one the
// caller names without having to rebuild a PromiseID from parts.
type state struct {
	created map[uint64]*golem.Promise[string]
}

var agent = golem.Implement(promises.Agent, func(promises.Id) *state {
	return &state{created: map[uint64]*golem.Promise[string]{}}
})

func init() {
	golem.Handle(agent, promises.Create, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		p := golem.NewPromise[string]()
		id := p.ID()
		ctx.State.created[id.OplogIndex] = p
		return int64(id.OplogIndex)
	})
	golem.Handle(agent, promises.Await, func(ctx *golem.Context[state], in promises.OplogIdxIn) string {
		p := ctx.State.created[uint64(in.OplogIdx)]
		if p == nil {
			panic(fmt.Errorf("no promise created with oplog index %d", in.OplogIdx))
		}
		return p.Await() // suspends until completed
	})
	golem.Handle(agent, promises.Complete, func(ctx *golem.Context[state], in promises.CompleteIn) bool {
		p := ctx.State.created[uint64(in.OplogIdx)]
		if p == nil {
			panic(fmt.Errorf("no promise created with oplog index %d", in.OplogIdx))
		}
		return golem.CompletePromise(p.ID(), in.Value)
	})
}
