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

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
)

// Flags are a set of independent booleans carried under names, lowering to the
// WIT flags type. Go's spelling is a struct whose exported fields are all bool:
//
//	type Perms struct{ Read, Write, Admin bool }
//
//	var _ = golem.DefineFlags[Perms]()
//
// Flag names are the field names with the first letter lower-cased, in
// declaration order — the same rule record fields follow. Order is the wire
// order, so reordering the fields is a breaking change.
//
// Without the registration the struct would lower to a record of bools, which
// is a different type on the wire.

// DefineFlags registers a struct of bool fields as a WIT flags set. Call it from
// a package-level var so registration happens before the component is invoked.
func DefineFlags[T any]() *engine.FlagsDef {
	return defineFlagsInto[T](defs)
}

// defineFlagsInto is the instance-scoped implementation behind DefineFlags.
func defineFlagsInto[T any](d *definitions) *engine.FlagsDef {
	return d.DefineFlags(reflect.TypeFor[T]())
}
