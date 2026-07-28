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
	"go/ast"
	"go/parser"
	"go/token"
	"path/filepath"
	"strings"
	"testing"
)

// TestDocCommentsStartWithIdentifier enforces the idiomatic Go form: a
// declaration's doc comment must begin with the identifier it documents (the
// form golint and GoLand require, e.g. "Foo does X", not "The Foo does X"). It
// parses every hand-written source file in this package — exported and
// unexported, including grouped declarations and tests — so the convention is
// kept in CI without relying on an editor. It intentionally does not require a
// comment to exist; it only checks the ones that do.
func TestDocCommentsStartWithIdentifier(t *testing.T) {
	files, err := filepath.Glob("*.go")
	if err != nil {
		t.Fatalf("globbing sources: %v", err)
	}
	fset := token.NewFileSet()
	for _, file := range files {
		af, err := parser.ParseFile(fset, file, nil, parser.ParseComments)
		if err != nil {
			t.Fatalf("parsing %s: %v", file, err)
		}
		for _, decl := range af.Decls {
			switch d := decl.(type) {
			case *ast.FuncDecl:
				checkDocForm(t, fset, d.Doc, d.Name.Name)
			case *ast.GenDecl:
				if d.Lparen.IsValid() {
					// Grouped: each spec carries its own doc comment.
					for _, spec := range d.Specs {
						if name, doc := specNameAndDoc(spec); name != "" {
							checkDocForm(t, fset, doc, name)
						}
					}
				} else {
					// Single: the GenDecl's doc documents the one spec.
					for _, spec := range d.Specs {
						if name, _ := specNameAndDoc(spec); name != "" {
							checkDocForm(t, fset, d.Doc, name)
						}
					}
				}
			}
		}
	}
}

// checkDocForm fails the test if doc exists and does not begin with name
// followed by a space (or equal name), matching golint's expected form.
func checkDocForm(t *testing.T, fset *token.FileSet, doc *ast.CommentGroup, name string) {
	if doc == nil {
		return
	}
	text := strings.TrimSpace(doc.Text())
	if text == name || strings.HasPrefix(text, name+" ") {
		return
	}
	first := text
	if i := strings.IndexByte(first, '\n'); i >= 0 {
		first = first[:i]
	}
	t.Errorf("%s: doc comment on %q must start with %q, got: %q", fset.Position(doc.Pos()), name, name+" ", first)
}

// specNameAndDoc returns the declared name and doc comment of a type or value
// spec, or "" for anything else (e.g. imports).
func specNameAndDoc(spec ast.Spec) (string, *ast.CommentGroup) {
	switch s := spec.(type) {
	case *ast.TypeSpec:
		return s.Name.Name, s.Doc
	case *ast.ValueSpec:
		if len(s.Names) > 0 {
			return s.Names[0].Name, s.Doc
		}
	}
	return "", nil
}
