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
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"reflect"
	"testing"
)

func TestBindAdapters(t *testing.T) {
	type S struct{ n int }
	ctx := &Context[S]{State: &S{}}

	add := Adapt(func(s *S, in int) int { s.n += in; return s.n })
	if got := add(ctx, 5); got != 5 || ctx.State.n != 5 {
		t.Fatalf("Adapt = %d, state %d", got, ctx.State.n)
	}
	get := Adapt0(func(s *S) int { return s.n })
	if got := get(ctx, Unit{}); got != 5 {
		t.Fatalf("Adapt0 = %d", got)
	}
	set := AdaptUnit(func(s *S, in int) { s.n = in })
	if set(ctx, 9); ctx.State.n != 9 {
		t.Fatalf("AdaptUnit state = %d", ctx.State.n)
	}
	clear := Adapt0Unit(func(s *S) { s.n = 0 })
	if clear(ctx, Unit{}); ctx.State.n != 0 {
		t.Fatalf("Adapt0Unit state = %d", ctx.State.n)
	}
}

func TestStructFieldsAndSchemaName(t *testing.T) {
	if fs := defs.StructFields(reflect.TypeFor[int]()); len(fs) != 0 {
		t.Fatalf("non-struct should yield no fields, got %d", len(fs))
	}
	type withUnexported struct {
		Exported   string
		unexported int //nolint:unused // present to exercise the skip path
	}
	_ = withUnexported{}.unexported
	fs := defs.StructFields(reflect.TypeFor[withUnexported]())
	if len(fs) != 1 || fs[0].Name != "exported" {
		t.Fatalf("fields = %+v", fs)
	}
	for in, want := range map[string]string{
		"": "", "X": "x", "Name": "name", "ID": "id", "APIKey": "apiKey",
		"UserID": "userID", "URLPath": "urlPath", "HTTP2Server": "http2Server", "AmountCents": "amountCents",
	} {
		if got := engine.SchemaName(in); got != want {
			t.Errorf("SchemaName(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestImplementRegistersMethods(t *testing.T) {
	type Id struct{ Name string }
	type St struct{ n int64 }
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "Counter"})
		type AddIn struct{ N int64 }
		add := def.Method[AddIn, int64]("add", Desc("adds to the counter"))
		get := def.Method[Unit, int64]("get")

		// Implement binds the constructor + returns the handle; Handle registers each
		// method, In/Out inferred from the handler and tied to the agent's Id + St.
		impl := implementInto[Id, St, NoConfig](d, def, simpleNewState[Id, St](func(Id) *St { return &St{} }), false)
		impl.Handle(add, func(ctx *Context[St], in AddIn) int64 { ctx.State.n += in.N; return ctx.State.n })
		impl.Handle(get, Adapt0(func(s *St) int64 { return s.n })) // method-expression style

		e := d.agents["Counter"]
		if e == nil || e.methods["add"] == nil || e.methods["get"] == nil {
			t.Fatal("Handle did not register the handlers under the agent")
		}
		if e.methods["add"].desc != "adds to the counter" {
			t.Fatalf("desc = %q", e.methods["add"].desc)
		}
		noDefErrs(t, d)
	})
}

func TestHandleRejectsDuplicateMethod(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A"})
		m := def.Method[Unit, Unit]("m")
		h := func(*Context[St], Unit) Unit { return Unit{} }
		impl := implementInto[Id, St, NoConfig](d, def, simpleNewState[Id, St](func(Id) *St { return &St{} }), false)
		impl.Handle(m, h)
		impl.Handle(m, h)
		mustDefErr(t, d, "method already implemented")
	})
}

func TestImplementRejectsSecondImplementAndNilInit(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	newState := simpleNewState[Id, St](func(Id) *St { return &St{} })
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A"})
		implementInto[Id, St, NoConfig](d, def, newState, false)
		implementInto[Id, St, NoConfig](d, def, newState, false) // second time
		mustDefErr(t, d, "already implemented")
	})
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "B"})
		implementInto[Id, St, NoConfig](d, def, nil, true) // nil init
		mustDefErr(t, d, "non-nil init")
	})
}

func TestRegistrationErrorsAreRecorded(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		defineAgentInto[Id, NoConfig](d, Spec{}) // empty Spec.Name
		// Implement against an agent that was never defined.
		implementInto[Id, St, NoConfig](d, &AgentDefinition[Id, NoConfig]{name: "does-not-exist"},
			simpleNewState[Id, St](func(Id) *St { return &St{} }), false)
		mustDefErr(t, d, "non-empty Spec.Name")
		mustDefErr(t, d, "unknown agent")
	})
}

func TestPromptHintsArePublished(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[Id, NoConfig](d, Spec{Name: "Hinted", PromptHint: "create one per user"})
		m := def.Method[Unit, Unit]("m", PromptHint("call when the user asks"))
		plain := def.Method[Unit, Unit]("plain")
		impl := implementInto[Id, St, NoConfig](d, def, simpleNewState[Id, St](func(Id) *St { return &St{} }), false)
		impl.Handle(m, func(*Context[St], Unit) Unit { return Unit{} })
		impl.Handle(plain, func(*Context[St], Unit) Unit { return Unit{} })
		types, errs := d.discover()
		if len(errs) != 0 || len(types) != 1 {
			t.Fatalf("types=%d errs=%v", len(types), errs)
		}
		at := types[0]
		if at.Constructor.PromptHint.IsNone() || at.Constructor.PromptHint.Some() != "create one per user" {
			t.Errorf("constructor hint %+v", at.Constructor.PromptHint)
		}
		for _, method := range at.Methods {
			switch method.Name {
			case "m":
				if method.PromptHint.IsNone() || method.PromptHint.Some() != "call when the user asks" {
					t.Errorf("method hint %+v", method.PromptHint)
				}
			case "plain":
				if method.PromptHint.IsSome() {
					t.Errorf("an unset hint was published: %+v", method.PromptHint)
				}
			}
		}
	})
}
