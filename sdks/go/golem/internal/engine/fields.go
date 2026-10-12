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
	"strings"
	"unicode"
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

// SchemaName is the schema name of an exported Go identifier, in the lowerCamelCase
// that the other SDKs use. The identifier splits into words at its case changes,
// and an initialism is one word, with a plural "s" (IDs). The first word is
// lower-cased whole, and each later word keeps only its first letter upper-case:
// ID is id, APIKey is apiKey, UserID is userId, URLPath is urlPath and UserIDs is
// userIds.
func SchemaName(s string) string {
	var b strings.Builder
	for i, word := range identifierWords([]rune(s)) {
		for j, r := range word {
			if i > 0 && j == 0 {
				b.WriteRune(unicode.ToUpper(r))
			} else {
				b.WriteRune(unicode.ToLower(r))
			}
		}
	}
	return b.String()
}

// identifierWords splits a Go identifier at its case changes. A new word starts
// at an upper-case letter after a lower-case letter or a digit, and at the last
// upper-case letter of a run that a lower-case letter follows (the P of URLPath),
// unless that lower-case letter is a lone plural "s" (the D of IDs).
func identifierWords(r []rune) [][]rune {
	var words [][]rune
	start := 0
	for i := 1; i < len(r); i++ {
		if !unicode.IsUpper(r[i]) {
			continue
		}
		prev := r[i-1]
		boundary := unicode.IsLower(prev) || unicode.IsDigit(prev)
		if unicode.IsUpper(prev) && i+1 < len(r) && unicode.IsLower(r[i+1]) {
			plural := r[i+1] == 's' && (i+2 == len(r) || !unicode.IsLower(r[i+2]))
			boundary = !plural
		}
		if boundary {
			words = append(words, r[start:i])
			start = i
		}
	}
	if start < len(r) {
		words = append(words, r[start:])
	}
	return words
}
