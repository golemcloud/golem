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

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
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

// flagsDef is the registered flag set for one struct type.
type flagsDef struct {
	typ    reflect.Type
	names  []string
	fields []int // struct field index per name, parallel to names
}

// DefineFlags registers a struct of bool fields as a WIT flags set. Call it from
// a package-level var so registration happens before the component is invoked.
func DefineFlags[T any]() *flagsDef {
	return defineFlagsInto[T](defs)
}

// defineFlagsInto is the instance-scoped implementation behind DefineFlags.
func defineFlagsInto[T any](d *definitions) *flagsDef {
	t := reflect.TypeFor[T]()
	fd := &flagsDef{typ: t}
	if t.Kind() != reflect.Struct {
		d.recordErr("", "", "DefineFlags requires a struct type, got %s (kind %s)", t, t.Kind())
		return fd
	}
	if _, dup := d.flags[t]; dup {
		d.recordErr("", "", "flags already defined for %s", t)
		return fd
	}
	for i := range t.NumField() {
		f := t.Field(i)
		if f.PkgPath != "" { // unexported
			continue
		}
		if f.Type.Kind() != reflect.Bool {
			d.recordErr("", "", "DefineFlags[%s]: field %s is %s, but every flag field must be bool",
				t, f.Name, f.Type)
			return fd
		}
		fd.names = append(fd.names, lowerFirst(f.Name))
		fd.fields = append(fd.fields, i)
	}
	if len(fd.names) == 0 {
		d.recordErr("", "", "DefineFlags[%s] needs at least one exported bool field", t)
	}
	d.flags[t] = fd
	return fd
}

// compileFlags lowers a registered flag struct to the WIT flags type: the type
// node carries the names, the value node one bool per name in the same order.
func compileFlags(c *codec, fd *flagsDef) {
	names := append([]string(nil), fd.names...)
	fields := fd.fields
	c.body = func(*graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyFlagsType(append([]string(nil), names...))
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		set := make([]bool, len(fields))
		for i, fi := range fields {
			set[i] = v.Field(fi).Bool()
		}
		return b.push(types.MakeSchemaValueNodeFlagsValue(set))
	}
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeFlagsValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		set := n.FlagsValue()
		if len(set) != len(fields) {
			return fmt.Errorf("%s: flags value has %d entries, but %s declares %d",
				c.typ, len(set), c.typ, len(fields))
		}
		for i, fi := range fields {
			dst.Field(fi).SetBool(set[i])
		}
		return nil
	}
}
