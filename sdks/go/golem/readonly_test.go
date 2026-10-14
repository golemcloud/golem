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
	"testing"
	"time"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// TestReadOnlyLowering — read-only methods lower to AgentMethod.ReadOnly with the
// right cache policy; a plain method stays read-write (None).
func TestReadOnlyLowering(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	h := func(*Context[St], Unit) Unit { return Unit{} }

	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A"})
		impl := implementInto[Id, St, NoConfig](d, def, simpleNewState[Id, St](func(Id) *St { return &St{} }), false)
		handle := func(name string, opts ...MethodOpt) {
			impl.Handle(def.Method[Unit, Unit](name, opts...), h)
		}
		handle("rw")             // read-write
		handle("ro", ReadOnly()) // default => until-write
		handle("nc", ReadOnly(NoCache()))
		handle("uw", ReadOnly(CacheUntilWrite()))
		handle("ttl", ReadOnly(CacheFor(30*time.Second)))

		types, errs := d.discover()
		if len(errs) != 0 {
			t.Fatalf("unexpected definition errors: %v", errs)
		}
		byName := map[string]common.AgentMethod{}
		for _, m := range types[0].Methods {
			byName[m.Name] = m
		}

		if byName["rw"].ReadOnly.IsSome() {
			t.Fatal("rw should be read-write (ReadOnly None)")
		}
		wantPolicy := func(name string, tag uint8) common.CachePolicy {
			m := byName[name]
			if !m.ReadOnly.IsSome() {
				t.Fatalf("%s should be read-only", name)
			}
			cp := m.ReadOnly.Some().CachePolicy
			if cp.Tag() != tag {
				t.Fatalf("%s cache tag = %d, want %d", name, cp.Tag(), tag)
			}
			return cp
		}
		wantPolicy("ro", common.CachePolicyUntilWrite)
		wantPolicy("nc", common.CachePolicyNoCache)
		wantPolicy("uw", common.CachePolicyUntilWrite)
		if cp := wantPolicy("ttl", common.CachePolicyTtl); cp.Ttl() != uint64(30*time.Second.Nanoseconds()) {
			t.Fatalf("ttl = %d ns, want %d", cp.Ttl(), uint64(30*time.Second.Nanoseconds()))
		}
	})
}

func TestReadOnlyMisuse(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	h := func(*Context[St], Unit) Unit { return Unit{} }

	// The descriptor is built inside the closure: a method descriptor comes from
	// the agent definition, which only exists once the test's definitions do.
	cases := []struct {
		name string
		mode Mode
		opts []MethodOpt
		want string
	}{
		{"repeated", Durable, []MethodOpt{ReadOnly(), ReadOnly()}, "ReadOnly set 2 times"},
		{"two policies", Durable, []MethodOpt{ReadOnly(NoCache(), CacheFor(time.Second))}, "at most one cache policy"},
		{"zero ttl", Durable, []MethodOpt{ReadOnly(CacheFor(0))}, "positive ttl"},
		{"negative ttl", Durable, []MethodOpt{ReadOnly(CacheFor(-time.Second))}, "positive ttl"},
		{"ephemeral", Ephemeral, []MethodOpt{ReadOnly()}, "only valid on a Durable agent"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			withDefs(t, func(d *definitions) {
				def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A", Mode: c.mode})
				impl := implementInto[Id, St, NoConfig](d, def, simpleNewState[Id, St](func(Id) *St { return &St{} }), false)
				impl.Handle(def.Method[Unit, Unit]("m", c.opts...), h)
				mustDefErr(t, d, c.want)
			})
		})
	}
}
