// Package cards is the DEFINITION of the permission-card scenario: an issuer
// that derives, inspects, lends, installs and revokes cards, and a receiver
// that reports the card authority it holds. The behaviour lives in cards/impl.
package cards

import "github.com/golemcloud/golem/sdks/go/golem"

type IssuerId struct{ Name string }

type ReceiverId struct{ Name string }

type ScenarioIn struct {
	// Receiver names the CardReceiver instance the scenario lends and installs
	// cards on.
	Receiver string
}

var Issuer = golem.DefineAgent[IssuerId](golem.Spec{
	Name: "CardIssuer", Description: "Derives, lends, installs and revokes permission cards", Mode: golem.Durable,
})

var Receiver = golem.DefineAgent[ReceiverId](golem.Spec{
	Name: "CardReceiver", Description: "Reports the permission-card authority it holds", Mode: golem.Durable,
})

var (
	// Scenario runs the permission-card scenario against a receiver and returns
	// one key=value observation per step. It keeps its scope card for
	// UseStaleScope.
	Scenario = Issuer.Method[ScenarioIn, []string]("scenario", golem.Description("Run the permission-card scenario"))
	// UseStaleScope attaches the scope card Scenario kept to a call in this
	// later invocation, derives a scope card from it, and releases it: one
	// key=value observation per step.
	UseStaleScope = Issuer.Method[golem.Unit, []string]("use-stale-scope", golem.Description("Use a scope card from an earlier invocation"))

	// Observe inspects every wallet card: "ok" when an inspection succeeds,
	// otherwise the error kind of the last refusal.
	Observe = Receiver.Method[golem.Unit, string]("observe", golem.Description("Report whether the receiver may inspect cards"))
	// Record appends what Observe reports to the receiver's log.
	Record = Receiver.Method[golem.Unit, golem.Unit]("record", golem.Description("Log whether the receiver may inspect cards"))
	// Log returns the observations Record logged.
	Log = Receiver.Method[golem.Unit, []string]("log", golem.Description("Return the logged observations"))
	// TryDeriveFromWallet derives from the wallet: "ok", or the error kind of
	// the refusal.
	TryDeriveFromWallet = Receiver.Method[golem.Unit, string]("try-derive-from-wallet", golem.Description("Report whether the receiver may derive from its wallet"))
)
