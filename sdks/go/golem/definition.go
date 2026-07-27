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
	"reflect"
	"strings"
	"sync"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// Definition errors are problems found while registering or finalizing agent
// definitions — a bad Spec, an unsupported field type, a conflicting NameType,
// an invalid HTTP route. They are *collected*, not panicked.
//
// The reason is structural: agents are declared from package-level vars, so a
// panic fires during init() — before the host can call any export — and surfaces
// as an opaque wasm trap with no message. Instead the SDK records every such
// problem and reports it through the export channels that do carry an error:
// discover-agent-types (what the CLI calls at deploy) and initialize. This is
// the general form of the rule "anything we cannot catch at compile time must be
// checked at runtime and reported during discovery."

// definitionError is one recorded problem, attributed as precisely as the point
// that found it allows.
type definitionError struct {
	agent  string // "" when not tied to a specific agent (e.g. NameType, a bad type)
	method string // "" when not method-specific
	detail string
}

func (e definitionError) Error() string {
	switch {
	case e.agent != "" && e.method != "":
		return fmt.Sprintf("golem: agent %q method %q: %s", e.agent, e.method, e.detail)
	case e.agent != "":
		return fmt.Sprintf("golem: agent %q: %s", e.agent, e.detail)
	default:
		return "golem: " + e.detail
	}
}

// defErrs accumulates definition errors. Registration runs single-goroutine at
// package init and the host calls exports single-threaded on this target, so no
// locking is needed (same reasoning as codecCache).
var defErrs []definitionError

// recordDefErr appends a definition error. agent/method may be "" when the
// problem is not attributable to one (e.g. a conflicting NameType).
func recordDefErr(agent, method, format string, args ...any) {
	defErrs = append(defErrs, definitionError{agent: agent, method: method, detail: fmt.Sprintf(format, args...)})
}

// agentDefErrors returns the joined details of the errors that block a given
// agent from being used — its own, plus any global (unattributed) ones — or ""
// if there are none.
func agentDefErrors(agent string) string {
	var msgs []string
	for _, e := range defErrs {
		if e.agent == "" || e.agent == agent {
			msgs = append(msgs, e.Error())
		}
	}
	return strings.Join(msgs, "\n")
}

// allDefErrors formats every collected error as one message for the wholesale
// discover-agent-types report.
func allDefErrors() string {
	msgs := make([]string, 0, len(defErrs))
	for _, e := range defErrs {
		msgs = append(msgs, "  - "+e.Error())
	}
	return fmt.Sprintf("component has %d agent definition error(s):\n%s", len(defErrs), strings.Join(msgs, "\n"))
}

// DefinitionErrors returns every problem found while building the component's
// agent definitions (bad specs, unsupported types, invalid HTTP routes, …).
// It is empty for a well-formed component. Intended for native tests, which can
// assert on definitions without deploying; at runtime the same errors surface
// through discover-agent-types and initialize.
func DefinitionErrors() []error {
	finalize()
	out := make([]error, len(defErrs))
	for i := range defErrs {
		out[i] = defErrs[i]
	}
	return out
}

// finalize derives every agent type once, recording (never panicking on) any
// problem. Deferred work — schema-graph construction and, for HTTP, route
// validation — happens here with full per-agent context, on top of the errors
// already recorded eagerly at each Define* call.
// finalizeOnce guards the one-time derivation. It is a *sync.Once (not a value)
// so tests can swap in a fresh Once to reset the guard without copying the lock.
var (
	finalizeOnce = &sync.Once{}
	cachedType   = map[string]common.AgentType{}
)

func finalize() {
	finalizeOnce.Do(func() {
		for _, name := range registryOrder {
			e := registry[name]
			at, invalids, err := safeBuildAgentType(e)
			if err != nil {
				recordDefErr(name, "", "%s", err)
				continue
			}
			for _, reason := range invalids {
				// reason already names the offending type (e.g. "int has a
				// platform-dependent width; …").
				recordDefErr(name, "", "%s", reason)
			}
			// HTTP mount/endpoints: validate and compile, recording problems and
			// patching the built type with the metadata the platform routes on.
			mount, endpoints, httpErrs := buildHTTP(e)
			defErrs = append(defErrs, httpErrs...)
			if mount.IsSome() {
				at.HttpMount = mount
				// Route collisions are checked only *within* an agent, where two
				// methods sharing a verb+path is unconditionally ambiguous. Cross
				// agent overlap depends on the httpApi deployment topology (agents
				// may be mounted under different subdomains), which the SDK does
				// not see — so, like the TS and Rust SDKs, that is left to the host
				// at deploy.
				routeOwners := map[string]string{}
				prefix := mount.Some().PathPrefix
				for _, mname := range e.order {
					for _, det := range endpoints[mname] {
						key := routeKey(det.HttpMethod, prefix, det.PathSuffix)
						if prev, seen := routeOwners[key]; seen {
							if prev == mname {
								recordDefErr(name, mname, "declares HTTP route %q more than once", key)
							} else {
								recordDefErr(name, mname, "HTTP route %q collides with method %q", key, prev)
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
			cachedType[name] = at
		}
	})
}

// safeBuildAgentType builds an agent's type metadata, converting any panic that
// slips through (an unconverted edge case in schema derivation) into a recorded
// error rather than a component-killing trap — the backstop for the no-trap rule.
func safeBuildAgentType(e *agentEntry) (at common.AgentType, invalids map[reflect.Type]string, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("deriving agent type panicked: %v", r)
		}
	}()
	at, invalids = buildAgentType(e)
	return at, invalids, nil
}
