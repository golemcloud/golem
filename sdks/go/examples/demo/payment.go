package main

import "github.com/golemcloud/golem/sdks/go/golem"

// Money is the shared value type: minor units plus a currency enum.
type Money struct {
	Amount   int64
	Currency Currency
}

// Currency is a WIT enum — a named integer whose case names are positional.
type Currency int32

const (
	EUR Currency = iota
	USD
)

var _ = golem.DefineEnum[Currency]("eur", "usd")

// PaymentResult is a WIT variant: Go has no sum types, so it is an interface
// plus a closed set of cases, each tagged with its wire name.
type PaymentResult interface{ isPaymentResult() }

type Approved struct{ TxnID string }
type Declined struct{ Reason string }

func (Approved) isPaymentResult() {}
func (Declined) isPaymentResult() {}

var _ = golem.DefineVariant[PaymentResult](
	golem.Case[Approved]("approved"),
	golem.Case[Declined]("declined"),
)

// PaymentAgent charges payments. It reads its config per key: a plain fee and a
// secret gateway key, both as typed descriptors.
type PaymentId struct{ Gateway string }
type PaymentState struct{ charged int64 }

var Payment = golem.DefineAgent[PaymentId, PaymentState](
	golem.Spec{Name: "PaymentAgent", Description: "Charges payments", Mode: golem.Durable},
	func(id PaymentId) *PaymentState { return &PaymentState{} },
)

var (
	// FeeCents is a local config value; GatewayKey is a secret. Callers can
	// override FeeCents per client with golem.WithConfigValue (see the shop).
	FeeCents   = golem.DefineConfig[int64](Payment, "fee", "cents")
	GatewayKey = golem.DefineSecret[string](Payment, "gateway", "key")
)

type ChargeIn struct{ Amount Money }

var Charge = golem.DefineMethod[PaymentId, ChargeIn, PaymentResult](
	"charge",
	golem.Desc("Charge an amount, returning approved or declined"),
)

func init() {
	golem.Implement(Payment, Charge, func(ctx *golem.Context[PaymentState], in ChargeIn) PaymentResult {
		// Read the secret (stays redacted in logs) and the fee from the host.
		key := golem.Must(GatewayKey.Get())
		fee := golem.Must(FeeCents.Get())

		if key.Reveal() == "" {
			return Declined{Reason: "no gateway key configured"}
		}
		if in.Amount.Amount <= fee {
			return Declined{Reason: "amount does not clear the fee"}
		}
		ctx.State.charged += in.Amount.Amount
		return Approved{TxnID: "txn-" + ctx.AgentID()}
	})
}
