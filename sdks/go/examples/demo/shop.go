package main

import (
	"encoding/binary"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// ShopConfig is declared as a whole struct: nested groups flatten into
// multi-segment paths (tax.ratePct) and are read back with LoadConfig. Secrets
// can live in a config struct too; here the shop instead declares its secret
// per-key (ShopApiKey) to show both styles side by side.
type ShopConfig struct {
	Greeting string
	Tax      TaxConfig
}
type TaxConfig struct{ RatePct int64 }

type ShopId struct{ Name string }

// ShopState keeps unexported counters, which reflection cannot see — so the shop
// implements Snapshotter to serialize them itself (Spec.Snapshot sets when).
type ShopState struct {
	orders  int64
	revenue int64
}

func (s *ShopState) Save() ([]byte, error) {
	b := make([]byte, 16)
	binary.LittleEndian.PutUint64(b[0:], uint64(s.orders))
	binary.LittleEndian.PutUint64(b[8:], uint64(s.revenue))
	return b, nil
}

func (s *ShopState) Load(b []byte) error {
	if len(b) >= 16 {
		s.orders = int64(binary.LittleEndian.Uint64(b[0:]))
		s.revenue = int64(binary.LittleEndian.Uint64(b[8:]))
	}
	return nil
}

var Shop = golem.DefineAgent[ShopId, ShopState](
	golem.Spec{
		Name:        "ShopAgent",
		Description: "A durable shop that checks out through the payment agent",
		Mode:        golem.Durable,
		// Config from a struct; read with golem.LoadConfig[ShopConfig].
		Config: golem.ConfigOf[ShopConfig](),
		// Snapshot the custom-serialized state every 5 invocations.
		Snapshot: golem.SnapshotEveryN(5),
		// Mount the methods under an HTTP prefix; {name} binds the Id's Name field.
		HTTP: &golem.Mount{Path: "/shops/{name}", CORS: []string{"*"}},
	},
	func(id ShopId) *ShopState { return &ShopState{} },
)

// A per-key secret on the same agent that uses struct config — declared here to
// show the descriptor style; read it with ShopApiKey.Get() where needed.
var ShopApiKey = golem.DefineSecret[string](Shop, "api", "key")

// PaymentMethod is a variant used as a method input.
type PaymentMethod interface{ isPaymentMethod() }

type Card struct {
	Last4 string
	Token golem.Secret[string] // never logged or persisted in the clear
}
type Cash struct{}

func (Card) isPaymentMethod() {}
func (Cash) isPaymentMethod() {}

var _ = golem.DefineVariant[PaymentMethod](
	golem.Case[Card]("card"),
	golem.Case[Cash]("cash"),
)

type Item struct {
	Sku        string
	Qty        int32
	Price      Money
	Attributes map[string]string // map<string, string>
}

type CheckoutIn struct {
	Items  []Item
	Method PaymentMethod
	Coupon *string // option<string>
}

type CheckoutResult struct {
	Total   Money
	Payment PaymentResult
}

type ShopStats struct {
	Orders  int64
	Revenue int64
}

var (
	Greet = golem.DefineMethod[ShopId, golem.Unit, string](
		"greet",
		golem.Desc("Greeting, from struct config"),
		golem.HTTP(golem.GET("/greet")),
	)
	Checkout = golem.DefineMethod[ShopId, CheckoutIn, CheckoutResult](
		"checkout",
		golem.Desc("Total the cart and charge it through the payment agent"),
		golem.HTTP(golem.POST("/checkout")),
	)
	Audit = golem.DefineMethod[ShopId, golem.Unit, []golem.Result[int64, string]](
		"audit",
		golem.Desc("Fan out to the regional ledgers concurrently"),
	)
	ScheduleReport = golem.DefineMethod[ShopId, golem.Unit, golem.Unit](
		"scheduleReport",
		golem.Desc("Schedule a ledger write for later"),
	)
	Stats = golem.DefineMethod[ShopId, golem.Unit, ShopStats]("stats")
)

func init() {
	golem.Implement(Shop, Greet, func(ctx *golem.Context[ShopState], _ golem.Unit) string {
		cfg := golem.Must(golem.LoadConfig[ShopConfig]())
		return cfg.Greeting
	})

	golem.Implement(Shop, Checkout, func(ctx *golem.Context[ShopState], in CheckoutIn) CheckoutResult {
		cfg := golem.Must(golem.LoadConfig[ShopConfig]())

		var subtotal int64
		for _, it := range in.Items {
			subtotal += it.Price.Amount * int64(it.Qty)
		}
		if in.Coupon != nil {
			subtotal -= 100 // flat demo discount when a coupon is present
		}
		total := Money{Amount: subtotal + subtotal*cfg.Tax.RatePct/100, Currency: EUR}

		// The payment method is a variant: cash settles locally, a card charges
		// the payment agent over RPC with a per-call fee override.
		var payment PaymentResult
		switch in.Method.(type) {
		case Cash:
			payment = Approved{TxnID: "cash"}
		default: // Card
			client := golem.Must(golem.ClientFor(
				Payment,
				PaymentId{Gateway: "stripe"},
				golem.WithConfigValue(FeeCents, int64(50)),
			))
			payment = golem.Must(Charge.Call(client, ChargeIn{Amount: total}))
		}

		ctx.State.orders++
		if _, ok := payment.(Approved); ok {
			ctx.State.revenue += total.Amount

			// Fire-and-forget: record the revenue in the regional ledger.
			ledger := golem.Must(golem.ClientFor(Ledger, LedgerId{Region: "eu-west"}))
			golem.Must(Record.Trigger(ledger, RecordIn{Amount: total.Amount}))

			// Track the checkout on a fresh ephemeral session (phantom client).
			if session, _, err := golem.NewPhantom(Session, SessionId{Label: "checkout"}); err == nil {
				golem.Must(Track.Call(session, TrackIn{Event: "checkout"}))
			}
		}
		return CheckoutResult{Total: total, Payment: payment}
	})

	golem.Implement(Shop, Audit, func(ctx *golem.Context[ShopState], _ golem.Unit) []golem.Result[int64, string] {
		regions := []string{"eu-west", "eu-central", "us-east"}

		// CallAsync returns immediately, so all three are in flight at once; a
		// goroutine blocked in Get yields to the component-model event loop.
		futures := make([]*golem.Future[golem.Result[int64, string]], 0, len(regions))
		for _, region := range regions {
			client := golem.Must(golem.ClientFor(Ledger, LedgerId{Region: region}))
			futures = append(futures, golem.Must(Record.CallAsync(client, RecordIn{Amount: 0})))
		}

		results := make([]golem.Result[int64, string], 0, len(futures))
		for _, f := range futures {
			results = append(results, golem.Must(f.Get()))
		}
		return results
	})

	golem.Implement(Shop, ScheduleReport, func(ctx *golem.Context[ShopState], _ golem.Unit) golem.Unit {
		ledger := golem.Must(golem.ClientFor(Ledger, LedgerId{Region: "eu-west"}))
		golem.Must(Record.Schedule(ledger, time.Now().Add(time.Hour), RecordIn{Amount: 0}))
		return golem.Unit{}
	})

	// A plain Go method bound with a method expression.
	golem.Implement(Shop, Stats, golem.Bind0((*ShopState).stats))
}

func (s *ShopState) stats() ShopStats {
	return ShopStats{Orders: s.orders, Revenue: s.revenue}
}
