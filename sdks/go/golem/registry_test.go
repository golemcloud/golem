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
	"reflect"
	"testing"
)

func TestBindAdapters(t *testing.T) {
	type S struct{ n int }
	ctx := &Context[S]{State: &S{}}

	add := Bind(func(s *S, in int) int { s.n += in; return s.n })
	if got := add(ctx, 5); got != 5 || ctx.State.n != 5 {
		t.Fatalf("Bind = %d, state %d", got, ctx.State.n)
	}
	get := Bind0(func(s *S) int { return s.n })
	if got := get(ctx, Unit{}); got != 5 {
		t.Fatalf("Bind0 = %d", got)
	}
	set := BindUnit(func(s *S, in int) { s.n = in })
	if set(ctx, 9); ctx.State.n != 9 {
		t.Fatalf("BindUnit state = %d", ctx.State.n)
	}
	clear := Bind0Unit(func(s *S) { s.n = 0 })
	if clear(ctx, Unit{}); ctx.State.n != 0 {
		t.Fatalf("Bind0Unit state = %d", ctx.State.n)
	}
}

func TestStructFieldsAndLowerFirst(t *testing.T) {
	if fs := defs.structFields(reflect.TypeFor[int]()); len(fs) != 0 {
		t.Fatalf("non-struct should yield no fields, got %d", len(fs))
	}
	type withUnexported struct {
		Exported   string
		unexported int //nolint:unused // present to exercise the skip path
	}
	_ = withUnexported{}.unexported
	fs := defs.structFields(reflect.TypeFor[withUnexported]())
	if len(fs) != 1 || fs[0].name != "exported" {
		t.Fatalf("fields = %+v", fs)
	}
	if lowerFirst("") != "" {
		t.Fatal("lowerFirst(\"\") should be empty")
	}
}

func TestMethodCombinator(t *testing.T) {
	type Id struct{ Name string }
	type St struct{ n int64 }
	withDefs(t, func(d *definitions) {
		a := defineAgentInto[Id, St](d, Spec{Name: "Counter"}, func(Id) *St { return &St{} })

		// One call declares the method, binds the handler, and returns the
		// descriptor for RPC — In/Out inferred from the handler, Id from the agent.
		add := methodInto[Id, St, NoConfig, int64, int64](d, a, "add",
			func(ctx *Context[St], in int64) int64 { ctx.State.n += in; return ctx.State.n },
			Desc("adds to the counter"))
		if add.name != "add" || add.desc != "adds to the counter" {
			t.Fatalf("returned descriptor = %+v", add)
		}

		// Composes with a Bind adapter, same as Implement.
		methodInto[Id, St, NoConfig, Unit, int64](d, a, "get",
			Bind0(func(s *St) int64 { return s.n }))

		// Both handlers are registered under the agent, and discovery is clean.
		e := d.agents["Counter"]
		if e == nil || e.methods["add"] == nil || e.methods["get"] == nil {
			t.Fatal("Method did not register the handlers under the agent")
		}
		noDefErrs(t, d)
	})
}

func TestMethodCombinatorRejectsDuplicate(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		a := defineAgentInto[Id, St](d, Spec{Name: "A"}, func(Id) *St { return &St{} })
		h := func(*Context[St], Unit) Unit { return Unit{} }
		methodInto[Id, St, NoConfig, Unit, Unit](d, a, "m", h)
		methodInto[Id, St, NoConfig, Unit, Unit](d, a, "m", h)
		mustDefErr(t, d, "method already implemented")
	})
}

func TestRegistrationErrorsAreRecorded(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		defineAgentInto[Id, St](d, Spec{}, func(Id) *St { return &St{} })
		implementInto[Id, St, NoConfig, Unit, Unit](d, &Agent[Id, St, NoConfig]{name: "does-not-exist"},
			MethodDef[Id, Unit, Unit]{name: "m"},
			func(*Context[St], Unit) Unit { return Unit{} })
		mustDefErr(t, d, "non-empty Spec.Name")
		mustDefErr(t, d, "unknown agent")
	})
}
