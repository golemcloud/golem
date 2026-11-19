// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"fmt"
	"iter"
	"time"

	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Agent management: reading agents' metadata, listing them, and updating,
// reverting and forking them. The host calls go through [hostOps], so the
// conversions and the filter logic here run natively in tests.

// AgentOperationErrorKind classifies an [AgentOperationError].
type AgentOperationErrorKind uint8

const (
	// AgentOperationPermissionDenied means the calling agent may not perform
	// the operation.
	AgentOperationPermissionDenied AgentOperationErrorKind = iota
	// AgentOperationBackendError means the host failed to carry it out.
	AgentOperationBackendError
)

// AgentOperationError is the failure of an operation on an agent: forking,
// updating, reverting, reading metadata or listing agents.
type AgentOperationError struct {
	Kind AgentOperationErrorKind
	// Message is the backend's message; empty for a denied permission.
	Message string
}

func (e *AgentOperationError) Error() string {
	if e.Kind == AgentOperationPermissionDenied {
		return "golem: agent operation: permission denied"
	}
	return "golem: agent operation: " + e.Message
}

func agentOperationErrorFromWit(w apiHost.AgentOperationError) error {
	switch w.Tag() {
	case apiHost.AgentOperationErrorPermissionDenied:
		return &AgentOperationError{Kind: AgentOperationPermissionDenied}
	case apiHost.AgentOperationErrorBackendError:
		return &AgentOperationError{Kind: AgentOperationBackendError, Message: w.BackendError()}
	default:
		panic(fmt.Sprintf("golem: unhandled agent-operation-error case %d", w.Tag()))
	}
}

// AgentStatus is an agent's execution status.
type AgentStatus uint8

const (
	// AgentStatusRunning: the agent is running an invocation.
	AgentStatusRunning AgentStatus = iota
	// AgentStatusIdle: the agent is ready for an invocation.
	AgentStatusIdle
	// AgentStatusSuspended: an invocation is waiting (sleeping, or on a promise).
	AgentStatusSuspended
	// AgentStatusInterrupted: the last invocation was interrupted and will resume.
	AgentStatusInterrupted
	// AgentStatusRetrying: the last invocation failed and a retry is scheduled.
	AgentStatusRetrying
	// AgentStatusFailed: the last invocation failed and the agent can no longer
	// be used.
	AgentStatusFailed
	// AgentStatusExited: the agent exited and can no longer be invoked.
	AgentStatusExited
)

var agentStatusNames = [...]string{"running", "idle", "suspended", "interrupted", "retrying", "failed", "exited"}

func (s AgentStatus) String() string {
	if int(s) < len(agentStatusNames) {
		return agentStatusNames[s]
	}
	return fmt.Sprintf("AgentStatus(%d)", uint8(s))
}

// AgentMetadata describes an agent as the host sees it.
type AgentMetadata struct {
	AgentID AgentID
	// AgentTypeName is the name of the agent's type. Owners without an agent
	// type have none.
	AgentTypeName Option[string]
	// Fingerprint is the incarnation of the agent: a deleted and re-created
	// agent keeps its ID but gets a new fingerprint.
	Fingerprint       UUID
	Args              []string
	Env               map[string]string
	Config            map[string]string
	Status            AgentStatus
	ComponentRevision uint64
	RetryCount        uint64
	EnvironmentID     UUID
}

func agentMetadataFromWit(w apiHost.AgentMetadata) AgentMetadata {
	agentTypeName := None[string]()
	if w.AgentTypeName.IsSome() {
		agentTypeName = Some(w.AgentTypeName.Some())
	}
	return AgentMetadata{
		AgentID:           agentIDFromWit(w.AgentId),
		AgentTypeName:     agentTypeName,
		Fingerprint:       uuidFromWit(w.Fingerprint),
		Args:              w.Args,
		Env:               pairsToMap(w.Env),
		Config:            pairsToMap(w.Config),
		Status:            AgentStatus(w.Status),
		ComponentRevision: w.ComponentRevision,
		RetryCount:        w.RetryCount,
		EnvironmentID:     uuidFromWit(w.EnvironmentId.Uuid),
	}
}

func pairsToMap(pairs []witTypes.Tuple2[string, string]) map[string]string {
	m := make(map[string]string, len(pairs))
	for _, p := range pairs {
		m[p.F0] = p.F1
	}
	return m
}

// GetSelfMetadata returns the running agent's metadata.
//
// It requires view permission on the running agent
// (agent(?agent) @ ?agent : view : *), admitted by the effective permission
// card and permitted by the initialCard upper bound; a denial is an
// [*AgentOperationError] of kind [AgentOperationPermissionDenied], and a host
// failure one of kind [AgentOperationBackendError]. The SDK does not call it on
// its own: initialization and snapshot loading read the agent's identity from
// GOLEM_AGENT_ID through the environment, which needs environment-read
// permission instead. The metadata holds the stored environment and
// configuration in full, without the per-key filtering of environment and
// configuration reads, so self-view is not an identity-only permission.
func GetSelfMetadata() (AgentMetadata, error) {
	w, err := hostOps.getSelfMetadata()
	if err != nil {
		return AgentMetadata{}, err
	}
	return agentMetadataFromWit(w), nil
}

func selfMetadataResult(res witTypes.Result[apiHost.AgentMetadata, apiHost.AgentOperationError]) (apiHost.AgentMetadata, error) {
	if res.IsErr() {
		return apiHost.AgentMetadata{}, agentOperationErrorFromWit(res.Err())
	}
	return res.Ok(), nil
}

// MustGetSelfMetadata is [GetSelfMetadata], panicking on failure.
func MustGetSelfMetadata() AgentMetadata { return must(GetSelfMetadata()) }

// GetAgentMetadata returns an agent's metadata, or false if there is no such
// agent.
func GetAgentMetadata(id AgentID) (AgentMetadata, bool) {
	w, ok := hostOps.getAgentMetadata(agentIDToWit(id))
	if !ok {
		return AgentMetadata{}, false
	}
	return agentMetadataFromWit(w), true
}

// GetAgentsOptions narrows and tunes [GetAgents]. The zero value lists every
// agent of the type, with metadata that may be slightly out of date.
type GetAgentsOptions struct {
	Filter AgentFilter
	// Precise makes the host compute each agent's latest state, which is
	// considerably more expensive.
	Precise bool
}

// GetAgents lists the agents of the named agent type in the current
// environment. Pages are fetched as the loop advances; a failing page is
// yielded as an error and ends the iteration.
//
//	for md, err := range golem.GetAgents("CounterAgent", golem.GetAgentsOptions{}) { … }
func GetAgents(agentTypeName string, opts GetAgentsOptions) iter.Seq2[AgentMetadata, error] {
	return func(yield func(AgentMetadata, error) bool) {
		pages := hostOps.getAgents(agentTypeName, opts.Filter.toWit(), opts.Precise)
		defer pages.close()
		for {
			page, more, err := pages.next()
			if err != nil {
				yield(AgentMetadata{}, err)
				return
			}
			if !more {
				return
			}
			for _, w := range page {
				if !yield(agentMetadataFromWit(w), nil) {
					return
				}
			}
		}
	}
}

// MustGetAgents is [GetAgents], panicking on a failing page.
func MustGetAgents(agentTypeName string, opts GetAgentsOptions) iter.Seq[AgentMetadata] {
	return func(yield func(AgentMetadata) bool) {
		for md, err := range GetAgents(agentTypeName, opts) {
			if err != nil {
				panic(err)
			}
			if !yield(md) {
				return
			}
		}
	}
}

// FilterComparator compares an agent's status, component revision or creation
// time in an [AgentFilter].
type FilterComparator uint8

const (
	FilterEqual FilterComparator = iota
	FilterNotEqual
	FilterGreaterEqual
	FilterGreater
	FilterLessEqual
	FilterLess
)

// StringFilterComparator compares an agent's key, environment or config
// variable in an [AgentFilter].
type StringFilterComparator uint8

const (
	StringFilterEqual StringFilterComparator = iota
	StringFilterNotEqual
	StringFilterLike
	StringFilterNotLike
	StringFilterStartsWith
)

// AgentFilter selects agents for [GetAgents]. Build one from the Agent…Filter
// conditions and combine them with And and Or; the zero value selects every
// agent.
//
//	golem.AgentKeyFilter(golem.StringFilterStartsWith, "order-").
//	    And(golem.AgentStatusFilter(golem.FilterEqual, golem.AgentStatusRunning))
type AgentFilter struct {
	// anyOf is the filter in disjunctive normal form: it matches an agent when
	// every condition of at least one group does. nil matches everything.
	anyOf [][]apiHost.AgentPropertyFilter
}

func condition(c apiHost.AgentPropertyFilter) AgentFilter {
	return AgentFilter{anyOf: [][]apiHost.AgentPropertyFilter{{c}}}
}

// AgentKeyFilter matches on the canonical key of a keyed agent. An agent
// without a key never matches it.
func AgentKeyFilter(cmp StringFilterComparator, value string) AgentFilter {
	return condition(apiHost.MakeAgentPropertyFilterKey(apiHost.AgentKeyFilter{Comparator: uint8(cmp), Value: value}))
}

// AgentStatusFilter matches on the agent's status.
func AgentStatusFilter(cmp FilterComparator, value AgentStatus) AgentFilter {
	return condition(apiHost.MakeAgentPropertyFilterStatus(apiHost.AgentStatusFilter{Comparator: uint8(cmp), Value: uint8(value)}))
}

// AgentVersionFilter matches on the component revision the agent runs.
func AgentVersionFilter(cmp FilterComparator, revision uint64) AgentFilter {
	return condition(apiHost.MakeAgentPropertyFilterVersion(apiHost.AgentVersionFilter{Comparator: uint8(cmp), Value: revision}))
}

// AgentCreatedAtFilter matches on when the agent was created.
func AgentCreatedAtFilter(cmp FilterComparator, t time.Time) AgentFilter {
	return condition(apiHost.MakeAgentPropertyFilterCreatedAt(apiHost.AgentCreatedAtFilter{Comparator: uint8(cmp), Value: uint64(t.UnixMilli())}))
}

// AgentEnvFilter matches on one of the agent's environment variables.
func AgentEnvFilter(name string, cmp StringFilterComparator, value string) AgentFilter {
	return condition(apiHost.MakeAgentPropertyFilterEnv(apiHost.AgentEnvFilter{Name: name, Comparator: uint8(cmp), Value: value}))
}

// AgentConfigFilter matches on one of the agent's config variables.
func AgentConfigFilter(name string, cmp StringFilterComparator, value string) AgentFilter {
	return condition(apiHost.MakeAgentPropertyFilterConfig(apiHost.AgentConfigVarsFilter{Name: name, Comparator: uint8(cmp), Value: value}))
}

// And matches agents both filters match.
func (f AgentFilter) And(other AgentFilter) AgentFilter {
	if f.anyOf == nil {
		return other
	}
	if other.anyOf == nil {
		return f
	}
	out := make([][]apiHost.AgentPropertyFilter, 0, len(f.anyOf)*len(other.anyOf))
	for _, a := range f.anyOf {
		for _, b := range other.anyOf {
			out = append(out, append(append([]apiHost.AgentPropertyFilter{}, a...), b...))
		}
	}
	return AgentFilter{anyOf: out}
}

// Or matches agents either filter matches.
func (f AgentFilter) Or(other AgentFilter) AgentFilter {
	if f.anyOf == nil || other.anyOf == nil {
		return AgentFilter{}
	}
	return AgentFilter{anyOf: append(append([][]apiHost.AgentPropertyFilter{}, f.anyOf...), other.anyOf...)}
}

func (f AgentFilter) toWit() witTypes.Option[apiHost.AgentAnyFilter] {
	if f.anyOf == nil {
		return witTypes.None[apiHost.AgentAnyFilter]()
	}
	groups := make([]apiHost.AgentAllFilter, 0, len(f.anyOf))
	for _, g := range f.anyOf {
		groups = append(groups, apiHost.AgentAllFilter{Filters: g})
	}
	return witTypes.Some(apiHost.AgentAnyFilter{Filters: groups})
}

// UpdateMode is how [UpdateAgent] moves an agent to another component
// revision.
type UpdateMode uint8

const (
	// UpdateModeAutomatic replays the agent on the new revision, and fails if
	// it diverges.
	UpdateModeAutomatic UpdateMode = iota
	// UpdateModeSnapshotBased saves the agent's state with its snapshot
	// function and loads it into the new revision.
	UpdateModeSnapshotBased
)

// UpdateAgent starts updating an agent to the given component revision. It
// returns once the request is accepted, without waiting for the update.
func UpdateAgent(id AgentID, revision uint64, mode UpdateMode) error {
	return hostOps.updateAgent(agentIDToWit(id), revision, uint8(mode))
}

// MustUpdateAgent is [UpdateAgent], panicking on failure.
func MustUpdateAgent(id AgentID, revision uint64, mode UpdateMode) {
	mustDo(UpdateAgent(id, revision, mode))
}

// RevertAgentTarget is how far [RevertAgent] rewinds an agent; make one with
// [RevertToOplogIndex] or [RevertLastInvocations].
type RevertAgentTarget struct{ wit apiHost.RevertAgentTarget }

// RevertToOplogIndex reverts to the given oplog index, the last entry kept.
func RevertToOplogIndex(index uint64) RevertAgentTarget {
	return RevertAgentTarget{apiHost.MakeRevertAgentTargetRevertToOplogIndex(index)}
}

// RevertLastInvocations reverts the agent's last n invocations.
func RevertLastInvocations(n uint64) RevertAgentTarget {
	return RevertAgentTarget{apiHost.MakeRevertAgentTargetRevertLastInvocations(n)}
}

// RevertAgent rewinds an agent's state.
func RevertAgent(id AgentID, target RevertAgentTarget) error {
	return hostOps.revertAgent(agentIDToWit(id), target.wit)
}

// MustRevertAgent is [RevertAgent], panicking on failure.
func MustRevertAgent(id AgentID, target RevertAgentTarget) { mustDo(RevertAgent(id, target)) }

// ForkAgent creates a new agent of source's type as a copy of source, with
// source's oplog up to and including cutOff, and returns the ID Golem issued
// for it. The new agent has no key.
func ForkAgent(source AgentID, cutOff uint64) (AgentID, error) {
	forked, err := hostOps.forkAgent(agentIDToWit(source), cutOff)
	if err != nil {
		return AgentID{}, err
	}
	return agentIDFromWit(forked), nil
}

// MustForkAgent is [ForkAgent], panicking on failure.
func MustForkAgent(source AgentID, cutOff uint64) AgentID { return must(ForkAgent(source, cutOff)) }

// ForkSelf copies the running agent into a new agent of the same type, with a
// new ID that Golem issues and no key. Both copies continue from here: forked
// tells them apart, and forkedID is the copy's ID in both.
func ForkSelf() (forked bool, forkedID AgentID, err error) { return hostOps.fork() }

// MustForkSelf is [ForkSelf], panicking on failure.
func MustForkSelf() (forked bool, forkedID AgentID) {
	forked, forkedID, err := ForkSelf()
	mustDo(err)
	return forked, forkedID
}

// ResolveComponentID looks a component up by reference: its name, or on Golem
// Cloud "project/component" or "account/project/component".
func ResolveComponentID(reference string) (UUID, bool) {
	w, ok := hostOps.resolveComponentID(reference)
	if !ok {
		return UUID{}, false
	}
	return uuidFromWit(w.Uuid), true
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func mustDo(err error) {
	if err != nil {
		panic(err)
	}
}
