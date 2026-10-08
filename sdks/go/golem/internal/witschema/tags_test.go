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

package witschema

import (
	"go/ast"
	"go/parser"
	"go/token"
	"strings"
	"testing"
)

// tagCount counts the uint8 tag constants named prefix<Case> in the core
// types bindings. Counting the source, rather than checking the last tag, also
// catches a case appended at the end.
func tagCount(t *testing.T, prefix string) int {
	t.Helper()
	file, err := parser.ParseFile(token.NewFileSet(), "../wit/golem_core_types/wit_bindings.go", nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	n := 0
	for _, decl := range file.Decls {
		gen, ok := decl.(*ast.GenDecl)
		if !ok || gen.Tok != token.CONST {
			continue
		}
		for _, spec := range gen.Specs {
			vs := spec.(*ast.ValueSpec)
			if id, ok := vs.Type.(*ast.Ident); !ok || id.Name != "uint8" {
				continue
			}
			for _, name := range vs.Names {
				if rest, ok := strings.CutPrefix(name.Name, prefix); ok && rest != "" && strings.ToUpper(rest[:1]) == rest[:1] {
					n++
				}
			}
		}
	}
	return n
}

func TestTheConvertersCoverEveryDeclaredTag(t *testing.T) {
	if got := tagCount(t, "SchemaTypeBody"); got != witBodyTagCount {
		t.Errorf("the bindings declare %d type tags, the converter pins %d", got, witBodyTagCount)
	}
	if got := tagCount(t, "SchemaValueNode"); got != witValueTagCount {
		t.Errorf("the bindings declare %d value tags, the converter pins %d", got, witValueTagCount)
	}
}
