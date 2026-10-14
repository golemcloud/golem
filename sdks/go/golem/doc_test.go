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
	"io/fs"
	"path/filepath"
	"strings"
	"testing"
)

// Every exported function and method of the public packages carries a doc
// comment where it is declared. pkg.go.dev and editors render the native build,
// so a function with one body per target is declared once, untagged, with its
// doc, and calls an unexported per-target body.
func TestEveryExportedFunctionIsDocumented(t *testing.T) {
	err := filepath.WalkDir(".", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			if path != "." && (strings.HasPrefix(path, "internal") || strings.HasPrefix(d.Name(), ".") || d.Name() == "wit") {
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(path, ".go") || strings.HasSuffix(path, "_test.go") {
			return nil
		}
		file, err := parser.ParseFile(token.NewFileSet(), path, nil, parser.ParseComments)
		if err != nil {
			return err
		}
		tagged := hasBuildConstraint(file)
		for _, decl := range file.Decls {
			fn, ok := decl.(*ast.FuncDecl)
			if !ok || !fn.Name.IsExported() {
				continue
			}
			if tagged && (fn.Recv == nil || exportedReceiver(fn.Recv.List[0].Type)) {
				t.Errorf("%s: %s is declared in a build-tagged file; declare it once, untagged, and call a per-target body", path, fn.Name.Name)
				continue
			}
			if fn.Doc != nil {
				continue
			}
			if fn.Recv != nil && (!exportedReceiver(fn.Recv.List[0].Type) || standardMethods[fn.Name.Name]) {
				continue
			}
			t.Errorf("%s: %s has no doc comment", path, fn.Name.Name)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
}

func hasBuildConstraint(file *ast.File) bool {
	for _, group := range file.Comments {
		if group.Pos() > file.Package {
			break
		}
		for _, c := range group.List {
			if strings.HasPrefix(c.Text, "//go:build") {
				return true
			}
		}
	}
	return false
}

// standardMethods implement the standard interfaces (error, fmt.Stringer,
// errors.Unwrap), whose meaning needs no doc of its own.
var standardMethods = map[string]bool{"Error": true, "String": true, "Unwrap": true}

func exportedReceiver(expr ast.Expr) bool {
	for {
		switch e := expr.(type) {
		case *ast.StarExpr:
			expr = e.X
		case *ast.IndexExpr:
			expr = e.X
		case *ast.IndexListExpr:
			expr = e.X
		case *ast.Ident:
			return e.IsExported()
		default:
			return true
		}
	}
}
