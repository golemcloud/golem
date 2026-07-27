// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import "reflect"

// pinnedTypeIDs overrides the derived type-id for specific Go types. Consulted
// by typeID during codec compilation.
var pinnedTypeIDs = map[reflect.Type]string{}

// NameType pins the language-independent `type-id` for T, overriding the id
// derived from its Go package path and name.
//
// A type-id only appears on the wire for a type that becomes a named definition
// — i.e. a recursive or otherwise ref-shared type (non-recursive types are
// inlined and carry no id). Cross-language interop requires every SDK to agree
// on that id for the same logical type; pin it here when the derived
// `pkg.path.TypeName` would not match the other side:
//
//	type Tree struct{ Children []*Tree }
//	var _ = golem.NameType[Tree]("myapp.tree")
//
// Call it from a package-level var so the pin is registered before any agent is
// compiled. Registering two different ids for the same type, or the same id for
// two types, panics — an ambiguous id is a wire-compatibility bug, not a
// tolerable one.
func NameType[T any](id string) struct{} {
	if id == "" {
		panic("golem: NameType requires a non-empty type-id")
	}
	t := reflect.TypeFor[T]()
	if existing, dup := pinnedTypeIDs[t]; dup && existing != id {
		panic("golem: type " + t.String() + " already pinned to type-id " + existing)
	}
	for other, existing := range pinnedTypeIDs {
		if existing == id && other != t {
			panic("golem: type-id " + id + " already pinned to " + other.String())
		}
	}
	pinnedTypeIDs[t] = id
	return struct{}{}
}
