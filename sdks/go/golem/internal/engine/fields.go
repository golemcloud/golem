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

package engine

import (
	"fmt"
	"reflect"
)

// Field is one exported struct field, in declaration order. Declaration
// order is the wire order: value trees encode records positionally.
type Field struct {
	Name  string
	Index int
	Typ   reflect.Type
	Codec *Codec
	// AutoInjected marks a Principal field: the host fills it from the
	// invocation, so it has no codec and no value on the wire.
	AutoInjected bool
	// Restrict is what the field's golem tag declared.
	Restrict *Restriction
}

// StructFields returns the exported fields of a struct type in declaration
// order. Non-structs (e.g. Unit) yield no fields.
func (d *Engine) StructFields(t reflect.Type) []Field {
	var out []Field
	if t == nil || t.Kind() != reflect.Struct {
		return out
	}
	for i := range t.NumField() {
		f := t.Field(i)
		if f.PkgPath != "" { // unexported
			continue
		}
		if f.Type == d.PrincipalType {
			out = append(out, Field{Name: SchemaName(f.Name), Index: i, Typ: f.Type, AutoInjected: true})
			continue
		}
		fi := Field{
			Name:  SchemaName(f.Name),
			Index: i,
			Typ:   f.Type,
			Codec: d.Compile(f.Type),
		}
		if tag, ok := f.Tag.Lookup("golem"); ok {
			r, err := ParseRestrictionTag(tag)
			if err == nil {
				err = d.CheckRestriction(fi.Codec, r)
			}
			if err != nil {
				fi.Codec = RestrictionFailed(fi.Codec, fmt.Sprintf("%s.%s: %v", t, f.Name, err))
			} else {
				fi.Restrict = r
			}
		}
		out = append(out, fi)
	}
	return out
}

// SchemaName is the schema name of an exported Go identifier: the identifier
// as Go would spell it unexported. The leading word is lower-cased whole, so an
// initialism stays one word: ID is id, APIKey is apiKey and UserID is userID.
func SchemaName(s string) string {
	b := []byte(s)
	n := 0
	for n < len(b) && b[n] >= 'A' && b[n] <= 'Z' {
		n++
	}
	// In URLPath the P starts the next word, so it stays upper-case.
	if n > 1 && n < len(b) && b[n] >= 'a' && b[n] <= 'z' {
		n--
	}
	for i := range n {
		b[i] += 'a' - 'A'
	}
	return string(b)
}
