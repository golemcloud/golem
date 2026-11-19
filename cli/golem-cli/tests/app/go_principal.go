// Package ledger is the Go principal fixture driven by go_principal.rs.
package ledger

import (
	"fmt"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type LedgerID struct{ Name string }

// WhoamiIn has no fields: the caller reaches the handler through its context.
type WhoamiIn struct{}

var Ledger = golem.DefineAgent[LedgerID](golem.Spec{Name: "LedgerAgent"})

// Whoami reports the caller of the invocation: its immediate principal as
// method=, its original principal as agent=.
var Whoami = Ledger.Method[WhoamiIn, string]("whoami")

type RelayID struct{ Name string }

type RelayIn struct{ Ledger string }

var Relay = golem.DefineAgent[RelayID](golem.Spec{Name: "RelayAgent"})

// Ask calls a ledger over RPC, so the ledger sees this agent as its principal.
var Ask = Relay.Method[RelayIn, string]("ask")

func describe(p golem.Principal) string {
	switch p := p.(type) {
	case golem.AgentPrincipal:
		return "agent:" + p.AgentID.String()
	case golem.GolemUserPrincipal:
		return "golem-user"
	case golem.OidcPrincipal:
		return "oidc:" + p.Sub
	case golem.AnonymousPrincipal:
		return "anonymous"
	default:
		return fmt.Sprintf("unexpected:%T", p)
	}
}

type ledgerState struct{}

type relayState struct{}

func init() {
	ledger := Ledger.Implement(func(LedgerID) *ledgerState { return &ledgerState{} })
	ledger.Handle(Whoami, func(ctx *golem.Context[ledgerState], _ WhoamiIn) string {
		caller := ctx.Caller()
		return fmt.Sprintf("method=%s agent=%s", describe(caller.Immediate), describe(caller.Original))
	})

	relay := Relay.Implement(func(RelayID) *relayState { return &relayState{} })
	relay.Handle(Ask, func(_ *golem.Context[relayState], in RelayIn) string {
		return Whoami.MustCall(Ledger.Upsert(LedgerID{Name: in.Ledger}), WhoamiIn{})
	})
}
