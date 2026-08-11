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
	"strings"
	"testing"
)

// withDefs runs fn against a fresh, fully isolated definition set. Because
// registration and discovery are both explicit over a *definitions, isolation is
// just a new instance — no global swapping, no guard, and safe to parallelize.
func withDefs(t *testing.T, fn func(d *definitions)) {
	t.Helper()
	fn(newDefinitions())
}

// mustDefErr asserts that discovering d surfaces at least one definition error
// mentioning want (registration- or derivation-phase).
func mustDefErr(t *testing.T, d *definitions, want string) {
	t.Helper()
	_, errs := d.discover()
	for _, e := range errs {
		if strings.Contains(e.Error(), want) {
			return
		}
	}
	t.Fatalf("expected a definition error mentioning %q; got %v", want, errs)
}

// noDefErrs asserts d discovers cleanly.
func noDefErrs(t *testing.T, d *definitions) {
	t.Helper()
	if _, errs := d.discover(); len(errs) != 0 {
		t.Fatalf("expected no definition errors, got %v", errs)
	}
}

// --- repeated / overwriting settings ---------------------------------------

func TestMisuseRepeatedDesc(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A"})
		m := DefineMethod[Id, Unit, Unit]("m", Desc("a"), Desc("b"))
		implementAgentInto[Id, St, NoConfig](d, def,
			simpleNewState[Id, St](func(Id) *St { return &St{} }), false,
			[]Binding[Id, St]{Bound(m, func(*Context[St], Unit) Unit { return Unit{} })})
		mustDefErr(t, d, "Desc set 2 times")
	})
}

func TestMisuseRepeatedEndpointAuth(t *testing.T) {
	e := agent("A", &Mount{Path: "/a/{id}"}, fields("id"),
		method("m", nil, POST("/x", EndpointAuth(true), EndpointAuth(false))))
	_, _, errs := buildHTTP(e)
	if !containsDefErr(errs, "EndpointAuth set 2 times") {
		t.Fatalf("errs = %v", errs)
	}
}

// --- distinct-identity requirements ----------------------------------------

func TestMisuseSharedIdType(t *testing.T) {
	type SharedID struct{ Name string }
	withDefs(t, func(d *definitions) {
		defineAgentInto[SharedID, NoConfig](d, Spec{Name: "A1"})
		defineAgentInto[SharedID, NoConfig](d, Spec{Name: "A2"})
		mustDefErr(t, d, "already used by agent")
	})
}

func TestMisuseDuplicateAgentName(t *testing.T) {
	type Id1 struct{ A string }
	type Id2 struct{ B string }
	withDefs(t, func(d *definitions) {
		defineAgentInto[Id1, NoConfig](d, Spec{Name: "Dup"})
		defineAgentInto[Id2, NoConfig](d, Spec{Name: "Dup"})
		mustDefErr(t, d, "already defined")
	})
}

func TestMisuseNonStructId(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineAgentInto[int, NoConfig](d, Spec{Name: "A"})
		mustDefErr(t, d, "must be a struct")
	})
}

func TestMisuseNilInit(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A"})
		implementAgentInto[Id, St, NoConfig](d, def, nil, true, nil)
		mustDefErr(t, d, "non-nil init")
	})
}

func TestMisuseNilHandler(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A"})
		implementAgentInto[Id, St, NoConfig](d, def,
			simpleNewState[Id, St](func(Id) *St { return &St{} }), false,
			[]Binding[Id, St]{Bound(DefineMethod[Id, Unit, Unit]("m"), (func(*Context[St], Unit) Unit)(nil))})
		mustDefErr(t, d, "non-nil handler")
	})
}

// --- method-level misuse ----------------------------------------------------

func TestMisuseEmptyMethodName(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A"})
		implementAgentInto[Id, St, NoConfig](d, def,
			simpleNewState[Id, St](func(Id) *St { return &St{} }), false,
			[]Binding[Id, St]{Bound(DefineMethod[Id, Unit, Unit](""), func(*Context[St], Unit) Unit { return Unit{} })})
		mustDefErr(t, d, "non-empty method name")
	})
}

func TestMisuseDuplicateMethod(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A"})
		m := DefineMethod[Id, Unit, Unit]("m")
		h := func(*Context[St], Unit) Unit { return Unit{} }
		implementAgentInto[Id, St, NoConfig](d, def,
			simpleNewState[Id, St](func(Id) *St { return &St{} }), false,
			[]Binding[Id, St]{Bound(m, h), Bound(m, h)})
		mustDefErr(t, d, "already implemented")
	})
}

// --- HTTP route collisions --------------------------------------------------

func TestMisuseDuplicateRoute(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	type In struct{ X string }
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A", HTTP: &Mount{Path: "/a/{name}"}})
		m1 := DefineMethod[Id, In, Unit]("m1", HTTP(GET("/dup/{x}")))
		m2 := DefineMethod[Id, In, Unit]("m2", HTTP(GET("/dup/{x}")))
		h := func(*Context[St], In) Unit { return Unit{} }
		implementAgentInto[Id, St, NoConfig](d, def,
			simpleNewState[Id, St](func(Id) *St { return &St{} }), false,
			[]Binding[Id, St]{Bound(m1, h), Bound(m2, h)})
		mustDefErr(t, d, "collides")
	})
}

// --- variant case type ------------------------------------------------------

type dupTypeVar interface{ dtv() }
type dupTypeImpl struct{}

func (dupTypeImpl) dtv() {}

func TestMisuseDuplicateCaseType(t *testing.T) {
	withDefs(t, func(d *definitions) {
		defineVariantInto[dupTypeVar](d, Case[dupTypeImpl]("a"), Case[dupTypeImpl]("b"))
		mustDefErr(t, d, "case type")
	})
}
