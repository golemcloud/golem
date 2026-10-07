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
	"errors"
	"fmt"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
	"reflect"
	"slices"
	"strings"

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// definitions holds everything built up as agents, methods, types and pins are
// declared. It is the explicit subject of both registration (the …Into helpers)
// and discovery ([discover]) — so a test builds a fresh instance, registers a
// controlled set into it, and dumps the result, with no package-global state.
//
// The public API (DefineAgent, …) registers into the single package-global
// [defs]; the exports read it back. Registration runs single-goroutine at
// package init and the host calls the exports single-threaded on this target, so
// the maps need no locking.
type definitions struct {
	*engine.Engine
	agents    map[string]*agentEntry
	order     []string
	idToAgent map[reflect.Type]string // Id type -> agent name, for typed calls (Get)
}

func newDefinitions() *definitions {
	return &definitions{
		Engine:    engine.New(principalType, sdkComposites...),
		agents:    map[string]*agentEntry{},
		idToAgent: map[reflect.Type]string{},
	}
}

// defs is the process-wide definition state the public API builds into.
//
// INVARIANT — keep the wrappers logic-free: the public registration functions
// (DefineAgent, Implement, DefineVariant, DefineEnum, DefineFlags, DefineUnion, NameType) MUST be one-line
// wrappers that forward to their `…Into(defs, …)` helper and nothing more. All
// logic — validation, error recording, codec compilation, discovery — lives in
// the instance-scoped helpers/methods so it runs against an explicit
// *definitions and is testable in isolation (see the withDefs test helper). Any
// logic added to a wrapper would execute only against this global and slip past
// the per-instance tests.
var defs = newDefinitions()

func init() {
	link.Engine = defs.Engine
	link.NewEngine = func() *engine.Engine { return newDefinitions().Engine }
	link.TypedValue = func(w types.TypedSchemaValue) any { return TypedValue{wit: w} }
	link.TypedValueWit = func(v any) types.TypedSchemaValue { return v.(TypedValue).wit }
	link.AgentError = agentErrorToGo
	link.Principal = func(p common.Principal) any { return principalFromWit(p) }
	link.LocalAgentTypes = func() ([]common.AgentType, error) {
		found, errs := defs.discover()
		if len(errs) > 0 {
			return found, errors.New(engine.AllErrors(errs))
		}
		return found, nil
	}
	link.RemoteCallError = rpcErrorToGo
	link.ScheduledInvocation = func(agentID, idempotencyKey string, token *host.CancellationToken) any {
		return &ScheduledInvocation{ID: InvocationID{AgentID: agentID, IdempotencyKey: idempotencyKey}, token: token}
	}
}

// discover derives every agent type and collects every problem, purely: it reads
// d but does not mutate it, so it is idempotent and safe to call repeatedly. The
// returned errors are the registration-phase errors plus everything derivation
// finds (unsupported types, invalid HTTP routes, intra-agent route collisions).
func (d *definitions) discover() ([]common.AgentType, []engine.DefError) {
	errs := append([]engine.DefError(nil), d.Errs...)
	var types []common.AgentType

	for _, name := range d.order {
		e := d.agents[name]
		// An agent must be implemented, and have at least one method — otherwise a
		// worker initialized as it would hit a nil constructor. Surface that as a
		// definition error rather than a runtime panic.
		if e.newState == nil {
			errs = append(errs, engine.DefError{Agent: name, Detail: "agent defined but never implemented (call golem.Implement)"})
		} else if len(e.order) == 0 && e.router == nil { // a router may serve static files only
			errs = append(errs, engine.DefError{Agent: name, Detail: "agent implemented but has no methods (call golem.Handle)"})
		}
		at, invalids, err := d.safeBuildAgentType(e)
		if err != nil {
			errs = append(errs, engine.DefError{Agent: name, Detail: err.Error()})
			continue
		}
		for _, reason := range invalids {
			// reason already names the offending type (e.g. "int has a
			// platform-dependent width; …").
			errs = append(errs, engine.DefError{Agent: name, Detail: reason})
		}

		// HTTP mount/endpoints: validate and compile, patching the built type
		// with the metadata the platform routes on.
		mount, endpoints, httpErrs := buildHTTP(e)
		errs = append(errs, httpErrs...)
		if mount.IsSome() {
			at.HttpMount = mount
			// Route collisions are checked only *within* an agent, where two
			// methods sharing a verb+path is unconditionally ambiguous. Cross
			// agent overlap depends on the httpApi deployment topology (agents
			// may be mounted under different subdomains), which the SDK does not
			// see — so, like the TS and Rust SDKs, that is left to the host.
			routeOwners := map[string]string{}
			prefix := mount.Some().PathPrefix
			for _, mname := range e.order {
				for _, det := range endpoints[mname] {
					key := routeKey(det.HttpMethod, prefix, det.PathSuffix)
					if prev, seen := routeOwners[key]; seen {
						if prev == mname {
							errs = append(errs, engine.DefError{Agent: name, Method: mname, Detail: fmt.Sprintf("declares HTTP route %q more than once", key)})
						} else {
							errs = append(errs, engine.DefError{Agent: name, Method: mname, Detail: fmt.Sprintf("HTTP route %q collides with method %q", key, prev)})
						}
					} else {
						routeOwners[key] = mname
					}
				}
			}
		}
		for i := range at.Methods {
			if eps := endpoints[at.Methods[i].Name]; len(eps) > 0 {
				at.Methods[i].HttpEndpoint = eps
			}
		}
		types = append(types, at)
	}
	for i := range types {
		deps, depErrs := d.agentDependencies(types[i].TypeName, types)
		types[i].Dependencies = deps
		errs = append(errs, depErrs...)
	}
	return types, errs
}

// agentDependencies builds the dependency records of an agent from the
// published types of the agents it names.
func (d *definitions) agentDependencies(name string, published []common.AgentType) ([]common.AgentDependency, []engine.DefError) {
	e := d.agents[name]
	if e == nil {
		return nil, nil
	}
	var errs []engine.DefError
	out := make([]common.AgentDependency, 0, len(e.deps))
	for _, dep := range e.deps {
		i := slices.IndexFunc(published, func(t common.AgentType) bool { return t.TypeName == dep })
		if i < 0 {
			errs = append(errs, engine.DefError{Agent: name, Detail: fmt.Sprintf("depends on %s, which this component does not define", dep)})
			continue
		}
		at := published[i]
		out = append(out, common.AgentDependency{
			TypeName:    at.TypeName,
			Description: someIfSet(at.Description),
			Schema:      at.Schema,
			Constructor: at.Constructor,
			Methods:     at.Methods,
		})
	}
	return out, errs
}

// safeBuildAgentType builds an agent's type metadata, converting any panic that
// slips through (an unconverted edge case in schema derivation) into a recorded
// error rather than a component-killing trap — the backstop for the no-trap rule.
func (d *definitions) safeBuildAgentType(e *agentEntry) (at common.AgentType, invalids map[reflect.Type]string, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("deriving agent type panicked: %v", r)
		}
	}()
	at, invalids = d.buildAgentType(e)
	return at, invalids, nil
}

// agentDefErrors returns the joined details of the errors that block a given
// agent from being used — its own, plus any global (unattributed) ones — or ""
// if there are none.
func agentDefErrors(errs []engine.DefError, agent string) string {
	var msgs []string
	for _, e := range errs {
		if e.Agent == "" || e.Agent == agent {
			msgs = append(msgs, e.Error())
		}
	}
	return strings.Join(msgs, "\n")
}

// DefinitionErrors returns every problem found while building the component's
// agent and tool definitions (bad specs, unsupported types, invalid HTTP
// routes, unbound tool arguments, …).
// It is empty for a well-formed component. Intended for native tests, which can
// assert on definitions without deploying; at runtime the same errors surface
// through discover-agent-types and initialize.
func DefinitionErrors() []error {
	// Tool and middleware problems are found as their metadata is derived,
	// which records them alongside the agents' own.
	if link.DiscoverTools != nil {
		link.DiscoverTools()
	}
	_, ds := defs.discover()
	out := make([]error, len(ds))
	for i := range ds {
		out[i] = ds[i]
	}
	return out
}

func someIfSet(s string) witTypes.Option[string] {
	if s == "" {
		return witTypes.None[string]()
	}
	return witTypes.Some(s)
}
