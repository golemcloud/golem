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

import "reflect"

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
// Thin wrapper over the package-global defs — keep all logic in [nameTypeInto]
// so it stays testable against an explicit *definitions. See [defs].
func NameType[T any](id string) struct{} {
	return nameTypeInto[T](defs, id)
}

// nameTypeInto is the instance-scoped implementation behind NameType.
func nameTypeInto[T any](d *definitions, id string) struct{} {
	t := reflect.TypeFor[T]()
	if id == "" {
		d.recordErr("", "", "NameType[%s] requires a non-empty type-id", t)
		return struct{}{}
	}
	if existing, dup := d.pins[t]; dup && existing != id {
		d.recordErr("", "", "type %s already pinned to type-id %q", t, existing)
		return struct{}{}
	}
	for other, existing := range d.pins {
		if existing == id && other != t {
			d.recordErr("", "", "type-id %q already pinned to %s", id, other)
			return struct{}{}
		}
	}
	d.pins[t] = id
	return struct{}{}
}
