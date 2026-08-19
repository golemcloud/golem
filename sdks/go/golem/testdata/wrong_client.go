//go:build ignore

// This file is DELIBERATELY uncompilable. It is not part of the package build
// (the `ignore` build tag excludes it); the compile_fail test invokes `go build`
// on it and asserts the type-checker rejects it.
//
// The guarantee under test: a MethodDef[Id, In, Out] can only be Called with a
// Client[Id] for the SAME agent identity. Aiming a payment method at an order
// client must be a COMPILE error, not a runtime not-found.
package main

import "github.com/golemcloud/golem/sdks/go/golem"

type PaymentID struct{ Merchant string }
type ChargeIn struct{ AmountCents int64 }

type OrderID struct{ Number string }

var Payment = golem.DefineAgent[PaymentID](golem.Spec{Name: "PaymentAgent"})

var Order = golem.DefineAgent[OrderID](golem.Spec{Name: "OrderAgent"})

// Charge is a method of the PAYMENT agent (its Id is PaymentID).
var Charge = golem.DefineMethod[PaymentID, ChargeIn, int64]("charge")

func main() {
	// A client for the ORDER agent — Client[OrderID].
	orderClient := Order.Get(OrderID{Number: "o-1"})

	// ERROR: Charge is MethodDef[PaymentID, …]; Call wants Client[PaymentID],
	// but orderClient is Client[OrderID]. Type-check must reject this.
	_ = Charge.Call(orderClient, ChargeIn{AmountCents: 100})
}
