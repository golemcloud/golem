//go:build ignore

// This file is DELIBERATELY uncompilable. It is not part of the package build
// (the `ignore` build tag excludes it); the compile_fail test invokes `go build`
// on it and asserts the type-checker rejects it.
//
// The guarantees under test: user code cannot build a permission card, cannot
// turn one card type into another, and can attach only a ScopeCard to a call.
package main

import (
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/reflection"
)

func main() {
	var id golem.CardID
	_ = golem.PermissionCard(id)

	_ = golem.PermissionCard{st: nil}

	var lookalike struct{ st *struct{ h any } }
	_ = golem.PolymorphicPermissionCard(lookalike)

	var card golem.PermissionCard
	var poly golem.PolymorphicPermissionCard
	var scope golem.ScopeCard
	_ = golem.PolymorphicPermissionCard(card)
	_ = golem.PermissionCard(poly)
	_ = golem.ScopeCard(card)
	_ = golem.PermissionCard(scope)

	var m golem.MethodDef[struct{}, golem.Unit, golem.Unit]
	var c golem.Client[struct{}]
	_ = m.WithScopeCard(card)
	_ = c.WithScopeCard(poly)
	_ = m.WithScopeCard(golem.Wallet()[0])
	var r *reflection.AgentClient
	_ = r.WithScopeCard(card)
	_ = reflection.CallScopeCard(poly)

	_, _ = scope.Revoke()
	_ = scope.Install(golem.Holder{})
	var _ golem.AnyPermissionCard = scope
}
