// Package impl is the IMPLEMENTATION of the failed-update reader test agent.
package impl

import (
	"fmt"
	"reflect"

	"agent-sdk-go/agents/divergence"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/oplog"
)

type state struct{}

var agent = divergence.Agent.Implement(func(divergence.Id) *state { return &state{} })

func init() {
	agent.Handle(divergence.Read, func(ctx *golem.Context[state], _ golem.Unit) divergence.Report {
		self := golem.MustGetSelfMetadata().AgentID
		report := divergence.Report{FromGet: []divergence.Divergence{}, FromSearch: []divergence.Divergence{}}
		index := uint64(1)
		for e := range oplog.MustGet(self, 1) {
			if e.Entry.Tag() == oplog.FailedUpdate {
				if d, ok := view(index, e.Entry, e.Attribution); ok {
					report.FromGet = append(report.FromGet, d)
				}
			}
			index++
		}
		for hit := range oplog.MustSearch(self, "failed-update") {
			if hit.Entry.Tag() == oplog.FailedUpdate {
				if d, ok := view(hit.Index, hit.Entry, hit.Attribution); ok {
					report.FromSearch = append(report.FromSearch, d)
				}
			}
		}
		return report
	})
}

// hostCallTag is the case tag of a host call step of replay-step.
const hostCallTag = 0

func view(index uint64, e oplog.Entry, attribution oplog.Attribution) (divergence.Divergence, bool) {
	params := e.FailedUpdate()
	if params.Divergence.IsNone() {
		return divergence.Divergence{}, false
	}
	d := params.Divergence.Some()
	mismatched := make([]uint8, 0, len(d.Mismatched))
	for _, field := range d.Mismatched {
		mismatched = append(mismatched, uint8(field))
	}
	var requests [2]any
	steps := [2]divergence.Step{{Kind: d.Expected.Tag()}, {Kind: d.Actual.Tag()}}
	for i, s := range [2]struct {
		tag  uint8
		call func() *divergence.Call
	}{
		{d.Expected.Tag(), func() *divergence.Call {
			h := d.Expected.HostCall()
			if h.Request.IsSome() {
				requests[0] = h.Request.Some()
			}
			return call(h.FunctionName, h.CarriesRequest, requests[0])
		}},
		{d.Actual.Tag(), func() *divergence.Call {
			h := d.Actual.HostCall()
			if h.Request.IsSome() {
				requests[1] = h.Request.Some()
			}
			return call(h.FunctionName, h.CarriesRequest, requests[1])
		}},
	} {
		if s.tag == hostCallTag {
			steps[i].Call = s.call()
		}
	}
	return divergence.Divergence{
		EntryIndex:     index,
		FailureClass:   uint8(params.FailureClass),
		OplogIndex:     d.OplogIndex,
		Expected:       steps[0],
		Actual:         steps[1],
		Mismatched:     mismatched,
		RequestsDiffer: !reflect.DeepEqual(requests[0], requests[1]),
		Attribution:    attribution.Tag(),
	}, true
}

func call(functionName string, carriesRequest bool, request any) *divergence.Call {
	out := &divergence.Call{FunctionName: functionName, CarriesRequest: carriesRequest}
	if request != nil {
		rendered := fmt.Sprintf("%v", request)
		out.Request = &rendered
	}
	return out
}
