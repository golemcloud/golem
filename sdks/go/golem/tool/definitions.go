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

// Package tool defines command-line tools that agents and other components
// call with typed arguments, calls them, and wraps them with middleware.
package tool

import (
	"reflect"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// definitions is the type system tool declarations are compiled against: the
// one the golem package's agents use, so a type means the same in both.
type definitions struct{ *engine.Engine }

// defs is the process-wide type system, shared with the golem package.
var defs = &definitions{Engine: link.Engine}

// newDefinitions is a fresh type system with the SDK's own types registered,
// for tests that must not share state.
func newDefinitions() *definitions { return &definitions{Engine: link.NewEngine()} }

func typedValue(w types.TypedSchemaValue) golem.TypedValue {
	return link.TypedValue(w).(golem.TypedValue)
}

func witOf(v golem.TypedValue) types.TypedSchemaValue { return link.TypedValueWit(v) }

// currentCaller returns a function that reads the caller of the running
// invocation on its first call, and a function that ends the call: with true
// when the call returned a stream, whose producer still reads the caller.
func currentCaller() (func() golem.Caller, func(bool)) {
	read, end := link.Caller()
	return read.(func() golem.Caller), end
}

// misplacedCaller is the build-time refusal of a live value in a command's arguments.
const misplacedCaller = "Principal and Caller cannot appear in a method, constructor or tool signature, or inside a value type. Read the caller with ctx.Caller() (golem.Caller), and store ctx.Caller().Original.Ref() as a PrincipalRef."

// isLiveValue reports a principal, a caller or a token provenance.
func isLiveValue(t reflect.Type) bool {
	principal := reflect.TypeFor[golem.Principal]()
	return t == principal || t == reflect.TypeFor[golem.Caller]() || t == reflect.TypeFor[golem.TokenProvenance]() ||
		(t.Kind() != reflect.Interface && t.Implements(principal))
}
