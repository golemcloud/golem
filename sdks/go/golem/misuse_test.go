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
	"strings"
	"sync"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

// withCleanRegistry runs fn against a fully empty registration slate — registry,
// id-type map, variant/enum tables, pins, collected errors and the finalize
// cache — restoring the package's real state afterwards. Registration-level
// misuse tests need this so DefineAgent/Implement don't collide with the
// package's own fixtures or leak into other tests.
func withCleanRegistry(t *testing.T, fn func()) {
	t.Helper()
	type snapshot struct {
		errs  []definitionError
		pins  map[reflect.Type]string
		reg   map[string]*agentEntry
		order []string
		idMap map[reflect.Type]string
		vars  map[reflect.Type]*variantDef
		enums map[reflect.Type]*enumDef
		once  *sync.Once // a pointer, so this never copies the lock
		cache map[string]common.AgentType
	}
	s := snapshot{defErrs, pinnedTypeIDs, registry, registryOrder, idTypeToAgent, variantRegistry, enumRegistry, finalizeOnce, cachedType}

	defErrs = nil
	pinnedTypeIDs = map[reflect.Type]string{}
	registry = map[string]*agentEntry{}
	registryOrder = nil
	idTypeToAgent = map[reflect.Type]string{}
	variantRegistry = map[reflect.Type]*variantDef{}
	enumRegistry = map[reflect.Type]*enumDef{}
	finalizeOnce = &sync.Once{}
	cachedType = map[string]common.AgentType{}

	t.Cleanup(func() {
		defErrs, pinnedTypeIDs = s.errs, s.pins
		registry, registryOrder, idTypeToAgent = s.reg, s.order, s.idMap
		variantRegistry, enumRegistry = s.vars, s.enums
		finalizeOnce, cachedType = s.once, s.cache
	})
	fn()
}

func hasError(errs []error, want string) bool {
	for _, e := range errs {
		if strings.Contains(e.Error(), want) {
			return true
		}
	}
	return false
}

// --- repeated / overwriting settings ---------------------------------------

func TestMisuseRepeatedDesc(t *testing.T) {
	withIsolatedDefs(t, func() {
		mustRecordDefErr(t, "Desc set 2 times", func() {
			DefineMethod[struct{}, struct{}, struct{}]("m", Desc("a"), Desc("b"))
		})
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
	type S1 struct{}
	type S2 struct{}
	withCleanRegistry(t, func() {
		DefineAgent[SharedID, S1](Spec{Name: "A1"}, func(SharedID) *S1 { return &S1{} })
		mustRecordDefErr(t, "already used by agent", func() {
			DefineAgent[SharedID, S2](Spec{Name: "A2"}, func(SharedID) *S2 { return &S2{} })
		})
	})
}

func TestMisuseDuplicateAgentName(t *testing.T) {
	type Id1 struct{ A string }
	type Id2 struct{ B string }
	type St struct{}
	withCleanRegistry(t, func() {
		DefineAgent[Id1, St](Spec{Name: "Dup"}, func(Id1) *St { return &St{} })
		mustRecordDefErr(t, "already defined", func() {
			DefineAgent[Id2, St](Spec{Name: "Dup"}, func(Id2) *St { return &St{} })
		})
	})
}

func TestMisuseNonStructId(t *testing.T) {
	type St struct{}
	withCleanRegistry(t, func() {
		mustRecordDefErr(t, "must be a struct", func() {
			DefineAgent[int, St](Spec{Name: "A"}, func(int) *St { return &St{} })
		})
	})
}

// --- method-level misuse ----------------------------------------------------

func TestMisuseEmptyMethodName(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withCleanRegistry(t, func() {
		a := DefineAgent[Id, St](Spec{Name: "A"}, func(Id) *St { return &St{} })
		mustRecordDefErr(t, "non-empty method name", func() {
			Implement(a, DefineMethod[Id, Unit, Unit](""), func(*Context[St], Unit) Unit { return Unit{} })
		})
	})
}

func TestMisuseDuplicateMethod(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	withCleanRegistry(t, func() {
		a := DefineAgent[Id, St](Spec{Name: "A"}, func(Id) *St { return &St{} })
		m := DefineMethod[Id, Unit, Unit]("m")
		h := func(*Context[St], Unit) Unit { return Unit{} }
		Implement(a, m, h)
		mustRecordDefErr(t, "already implemented", func() { Implement(a, m, h) })
	})
}

// --- HTTP route collisions --------------------------------------------------

func TestMisuseDuplicateRoute(t *testing.T) {
	type Id struct{ Name string }
	type St struct{}
	type In struct{ X string }
	withCleanRegistry(t, func() {
		a := DefineAgent[Id, St](Spec{Name: "A", HTTP: &Mount{Path: "/a/{name}"}},
			func(Id) *St { return &St{} })
		m1 := DefineMethod[Id, In, Unit]("m1", HTTP(GET("/dup/{x}")))
		m2 := DefineMethod[Id, In, Unit]("m2", HTTP(GET("/dup/{x}")))
		Implement(a, m1, func(*Context[St], In) Unit { return Unit{} })
		Implement(a, m2, func(*Context[St], In) Unit { return Unit{} })
		if errs := DefinitionErrors(); !hasError(errs, "collides") {
			t.Fatalf("expected a route-collision error, got %v", errs)
		}
	})
}

// --- variant case type ------------------------------------------------------

type dupTypeVar interface{ dtv() }
type dupTypeImpl struct{}

func (dupTypeImpl) dtv() {}

func TestMisuseDuplicateCaseType(t *testing.T) {
	withIsolatedDefs(t, func() {
		mustRecordDefErr(t, "case type", func() {
			DefineVariant[dupTypeVar](Case[dupTypeImpl]("a"), Case[dupTypeImpl]("b"))
		})
	})
}
