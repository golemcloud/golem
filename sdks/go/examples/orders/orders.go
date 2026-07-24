// Package main shows the Golem Go SDK's type vocabulary: nested options and
// results, a variant, an enum, and a secret — all derived from ordinary Go
// types, with no code generation and no schema DSL.
package main

import (
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// --- domain types -----------------------------------------------------------

type Money struct {
	Amount   int64 // minor units
	Currency string
}

// PaymentMethod is a WIT variant. Go has no sum types, so it is an interface
// plus a declared, closed set of cases; the unexported marker method keeps any
// other package from joining it.
type PaymentMethod interface{ isPaymentMethod() }

type Card struct {
	Last4 string
	Token golem.Secret[string] // never logged, never persisted in the clear
}
type Cash struct{}
type Transfer struct{ IBAN string }

func (Card) isPaymentMethod()     {}
func (Cash) isPaymentMethod()     {}
func (Transfer) isPaymentMethod() {}

var _ = golem.DefineVariant[PaymentMethod](
	golem.Case[Card]("card"),
	golem.Case[Cash]("cash"),
	golem.Case[Transfer]("transfer"),
)

// State is a WIT enum: a named integer plus its case names, positionally.
type State int32

const (
	StateOpen State = iota
	StatePaid
	StateRefunded
)

var _ = golem.DefineEnum[State]("open", "paid", "refunded")

type Line struct {
	Sku   string
	Qty   int32
	Price Money
}

// Order shows the nesting that motivated the design. Each field lowers to the
// WIT type in the comment; nothing here is annotated or generated.
type Order struct {
	ID       string
	State    State                                     // enum
	Lines    []Line                                    // list<record>
	Coupon   *string                                   // option<string>
	Method   PaymentMethod                             // variant
	Refund   golem.Option[golem.Result[Money, string]] // option<result<record, string>>
	Labels   map[string]string                         // map<string, string>
	PlacedAt time.Time                                 // datetime
	Deadline time.Duration                             // duration
}

// --- agent ------------------------------------------------------------------

type OrderId struct{ OrderID string }

type OrderState struct{ order Order }

type PlaceIn struct {
	Lines  []Line
	Method PaymentMethod
	Coupon *string
}

type RefundIn struct{ Amount Money }

var Orders = golem.DefineAgent[OrderId, OrderState](
	golem.Spec{Name: "OrderAgent", Description: "A durable order", Mode: golem.Durable},
	func(id OrderId) *OrderState {
		return &OrderState{order: Order{
			ID:       id.OrderID,
			State:    StateOpen,
			Lines:    []Line{},
			Labels:   map[string]string{},
			Refund:   golem.None[golem.Result[Money, string]](),
			PlacedAt: time.Now().UTC(),
			Deadline: 24 * time.Hour,
		}}
	},
)

var (
	Place  = golem.DefineMethod[OrderId, PlaceIn, Order]("place", golem.Desc("Place the order"))
	Get    = golem.DefineMethod[OrderId, golem.Unit, Order]("get")
	Refund = golem.DefineMethod[OrderId, RefundIn, Order]("refund", golem.Desc("Refund, recording success or failure as a value"))
	Audit  = golem.DefineMethod[OrderId, golem.Unit, []Money]("audit", golem.Desc("Fan out to the ledger agents"))
)

// --- a second agent, called over RPC --------------------------------------

type LedgerId struct{ Region string }
type LedgerState struct{ total Money }

var Ledger = golem.DefineAgent[LedgerId, LedgerState](
	golem.Spec{Name: "LedgerAgent", Description: "Per-region ledger", Mode: golem.Durable},
	func(id LedgerId) *LedgerState {
		return &LedgerState{total: Money{Currency: "EUR"}}
	},
)

var Record = golem.DefineMethod[LedgerId, RefundIn, Money]("record")

func init() {
	golem.Implement(Ledger, Record, func(ctx *golem.Context[LedgerState], in RefundIn) Money {
		ctx.State.total.Amount += in.Amount.Amount
		return ctx.State.total
	})
}

func init() {
	golem.Implement(Orders, Place, func(ctx *golem.Context[OrderState], in PlaceIn) Order {
		if len(in.Lines) == 0 {
			// A genuine failure: panic aborts the invocation and surfaces a
			// non-retriable error to the caller (the worker survives).
			panic("an order needs at least one line")
		}
		ctx.State.order.Lines = in.Lines
		ctx.State.order.Method = in.Method
		ctx.State.order.Coupon = in.Coupon
		ctx.State.order.State = StatePaid
		return ctx.State.order
	})

	golem.Implement(Orders, Refund, func(ctx *golem.Context[OrderState], in RefundIn) Order {
		// An expected, typed outcome: a *value*, not a failure — the caller sees
		// result<money, string> on a successful invocation, not an aborted one.
		if ctx.State.order.State != StatePaid {
			ctx.State.order.Refund = golem.Some(golem.Err[Money, string]("order is not paid"))
			return ctx.State.order
		}
		ctx.State.order.Refund = golem.Some(golem.Ok[Money, string](in.Amount))
		ctx.State.order.State = StateRefunded
		return ctx.State.order
	})
}

// snapshot is an ordinary Go method, bound with a method expression.
func (s *OrderState) snapshot() Order { return s.order }

func init() {
	golem.Implement(Orders, Audit, func(ctx *golem.Context[OrderState], _ golem.Unit) []Money {
		regions := []string{"eu-west", "eu-central", "us-east"}

		// Fan out. CallAsync returns immediately, so all three are in flight;
		// a goroutine blocked in Get yields to the component-model event loop.
		// (Concurrency is across DIFFERENT targets — one agent instance still
		// handles a single invocation at a time.) golem.Must turns the inner
		// (value, error) calls into a panic-on-error, aborting the invocation.
		futures := make([]*golem.Future[Money], 0, len(regions))
		for _, region := range regions {
			client := golem.Must(golem.ClientFor(Ledger, LedgerId{Region: region}))
			f := golem.Must(Record.CallAsync(client, RefundIn{Amount: Money{Amount: 1, Currency: "EUR"}}))
			futures = append(futures, f)
		}

		totals := make([]Money, 0, len(futures))
		for _, f := range futures {
			totals = append(totals, golem.Must(f.Get()))
		}
		return totals
	})

	// A plain Go method bound via a method expression.
	golem.Implement(Orders, Get, golem.Bind0((*OrderState).snapshot))
}

func main() {}
