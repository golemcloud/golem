//go:build ignore

// This file is DELIBERATELY uncompilable. It is not part of the package build
// (the `ignore` build tag excludes it); the compile_fail test invokes `go build`
// on it and asserts the type-checker rejects it.
//
// The guarantee under test: user code cannot build a permission card. A card
// wraps a host handle; neither an identifier nor a struct of its own shape
// turns into one.
package main

import "github.com/golemcloud/golem/sdks/go/golem"

func main() {
	var id golem.CardID
	_ = golem.PermissionCard(id)

	_ = golem.PermissionCard{st: nil}

	var lookalike struct{ st *struct{ h any } }
	_ = golem.PolymorphicPermissionCard(lookalike)
}
