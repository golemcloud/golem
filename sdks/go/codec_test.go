package golem

import (
	"fmt"
	"reflect"
	"strings"
	"testing"
	"time"

	common "github.com/golemcloud/golem/sdks/go/internal/wit/golem_agent_common"
	types "github.com/golemcloud/golem/sdks/go/internal/wit/golem_core_types"
)

// ---------------------------------------------------------------------------
// schema / value agreement
// ---------------------------------------------------------------------------

// checkAgreement walks a type graph and a value tree together and asserts every
// value node matches the schema node describing it — same kind, same arity.
//
// This is the property the codec design exists to guarantee. Encoding, decoding
// and schema derivation used to be three separate walks that had to agree by
// convention; now they are built together per type, and this check is what
// proves it for every case exercised below.
func checkAgreement(g types.SchemaGraph, sIdx int32, tree types.SchemaValueTree, vIdx int32, path string) error {
	if int(sIdx) >= len(g.TypeNodes) {
		return fmt.Errorf("%s: type index %d out of range", path, sIdx)
	}
	if int(vIdx) >= len(tree.ValueNodes) {
		return fmt.Errorf("%s: value index %d out of range", path, vIdx)
	}
	body := g.TypeNodes[sIdx].Body
	val := tree.ValueNodes[vIdx]

	mismatch := func() error {
		return fmt.Errorf("%s: value tag %d does not match schema tag %d", path, val.Tag(), body.Tag())
	}

	switch body.Tag() {
	case types.SchemaTypeBodyRecordType:
		if val.Tag() != types.SchemaValueNodeRecordValue {
			return mismatch()
		}
		fields, children := body.RecordType(), val.RecordValue()
		if len(fields) != len(children) {
			return fmt.Errorf("%s: record has %d value(s) but schema declares %d field(s)",
				path, len(children), len(fields))
		}
		for i, f := range fields {
			if err := checkAgreement(g, f.Body, tree, children[i], path+"."+f.Name); err != nil {
				return err
			}
		}
	case types.SchemaTypeBodyOptionType:
		if val.Tag() != types.SchemaValueNodeOptionValue {
			return mismatch()
		}
		if inner := val.OptionValue(); inner.IsSome() {
			return checkAgreement(g, body.OptionType(), tree, inner.Some(), path+"?")
		}
	case types.SchemaTypeBodyResultType:
		if val.Tag() != types.SchemaValueNodeResultValue {
			return mismatch()
		}
		spec, payload := body.ResultType(), val.ResultValue()
		if payload.Tag() == types.ResultValuePayloadErrValue {
			if c := payload.ErrValue(); c.IsSome() {
				return checkAgreement(g, spec.Err.Some(), tree, c.Some(), path+".err")
			}
		} else if c := payload.OkValue(); c.IsSome() {
			return checkAgreement(g, spec.Ok.Some(), tree, c.Some(), path+".ok")
		}
	case types.SchemaTypeBodyListType:
		if val.Tag() != types.SchemaValueNodeListValue {
			return mismatch()
		}
		for i, child := range val.ListValue() {
			if err := checkAgreement(g, body.ListType(), tree, child, fmt.Sprintf("%s[%d]", path, i)); err != nil {
				return err
			}
		}
	case types.SchemaTypeBodyFixedListType:
		if val.Tag() != types.SchemaValueNodeFixedListValue {
			return mismatch()
		}
		spec, children := body.FixedListType(), val.FixedListValue()
		if int(spec.Length) != len(children) {
			return fmt.Errorf("%s: fixed list has %d element(s), schema declares %d",
				path, len(children), spec.Length)
		}
		for i, child := range children {
			if err := checkAgreement(g, spec.Element, tree, child, fmt.Sprintf("%s[%d]", path, i)); err != nil {
				return err
			}
		}
	case types.SchemaTypeBodyMapType:
		if val.Tag() != types.SchemaValueNodeMapValue {
			return mismatch()
		}
		spec := body.MapType()
		for i, e := range val.MapValue() {
			if err := checkAgreement(g, spec.Key, tree, e.Key, fmt.Sprintf("%s.key%d", path, i)); err != nil {
				return err
			}
			if err := checkAgreement(g, spec.Value, tree, e.Value, fmt.Sprintf("%s.val%d", path, i)); err != nil {
				return err
			}
		}
	}
	return nil
}

// roundTrip encodes in, verifies the tree agrees with the derived schema, then
// decodes it back.
func roundTrip[T any](t *testing.T, in T) T {
	t.Helper()
	c := compile(reflect.TypeFor[T]())

	// &in, not in: reflect.ValueOf would unwrap an interface-typed T to its
	// concrete type, which is exactly what a variant must not lose.
	tree := encodeWith(c, reflect.ValueOf(&in).Elem())

	var g graphBuilder
	root := g.node(c)
	if err := checkAgreement(g.build(), root, tree, tree.Root, reflect.TypeFor[T]().String()); err != nil {
		t.Fatalf("schema/value disagreement: %v", err)
	}

	out := reflect.New(reflect.TypeFor[T]()).Elem()
	d := decoder{nodes: tree.ValueNodes}
	if err := c.decode(&d, out, tree.Root); err != nil {
		t.Fatalf("decode failed: %v", err)
	}
	return out.Interface().(T)
}

func assertRoundTrip[T any](t *testing.T, name string, in T) {
	t.Helper()
	if got := roundTrip(t, in); !reflect.DeepEqual(got, in) {
		t.Errorf("%s: round trip changed the value\n got: %#v\nwant: %#v", name, got, in)
	}
}

// ---------------------------------------------------------------------------
// types under test
// ---------------------------------------------------------------------------

type Money struct {
	Amount   int64
	Currency string
}

type Line struct {
	Sku string
	Qty int32
}

type Order struct {
	ID     string
	Coupon *string
	Lines  []Line
	Refund Option[Result[Money, string]]
	Tags   map[string]int64
	Digits [3]uint8
}

func TestRoundTripPrimitivesAndRecords(t *testing.T) {
	assertRoundTrip(t, "string", "hello")
	assertRoundTrip(t, "bool", true)
	assertRoundTrip(t, "int64", int64(-9000))
	assertRoundTrip(t, "uint8", uint8(255))
	assertRoundTrip(t, "float64", 3.5)
	assertRoundTrip(t, "record", Money{Amount: 1050, Currency: "EUR"})
}

func TestRoundTripNestedComposites(t *testing.T) {
	coupon := "SUMMER"
	full := Order{
		ID:     "ord-1",
		Coupon: &coupon,
		Lines:  []Line{{Sku: "a", Qty: 2}, {Sku: "b", Qty: 1}},
		Refund: Some(Ok[Money, string](Money{Amount: 250, Currency: "EUR"})),
		Tags:   map[string]int64{"priority": 1, "region": 7},
		Digits: [3]uint8{1, 2, 3},
	}
	assertRoundTrip(t, "order (all present)", full)

	// The absent/failed shapes must round-trip too.
	empty := Order{
		ID:     "ord-2",
		Coupon: nil,
		Lines:  []Line{},
		Refund: None[Result[Money, string]](),
		Tags:   map[string]int64{},
		Digits: [3]uint8{},
	}
	assertRoundTrip(t, "order (all absent)", empty)

	failed := full
	failed.Refund = Some(Err[Money, string]("card expired"))
	assertRoundTrip(t, "order (result is err)", failed)
}

func TestRoundTripDeeplyNestedOptionAndResult(t *testing.T) {
	// The shapes that motivated the codec design: options of results, results of
	// options, options of options, and lists of each.
	assertRoundTrip(t, "option<option<string>>", Some(Some("x")))
	assertRoundTrip(t, "option<option<string>> inner none", Some(None[string]()))
	assertRoundTrip(t, "option<option<string>> outer none", None[Option[string]]())
	assertRoundTrip(t, "**string via pointers", func() **string {
		s := "deep"
		p := &s
		return &p
	}())
	assertRoundTrip(t, "result<option<Money>, string>",
		Ok[Option[Money], string](Some(Money{Amount: 1, Currency: "GBP"})))
	assertRoundTrip(t, "result<result<..>>",
		Ok[Result[int64, string], string](Err[int64, string]("inner")))
	assertRoundTrip(t, "list<option<result<..>>>", []Option[Result[Money, string]]{
		Some(Ok[Money, string](Money{Amount: 5, Currency: "USD"})),
		None[Result[Money, string]](),
		Some(Err[Money, string]("nope")),
	})
	assertRoundTrip(t, "map<string, list<option<int64>>>", map[string][]Option[int64]{
		"a": {Some(int64(1)), None[int64]()},
		"b": {},
	})
}

// *T and Option[T] are two spellings of the same thing, so they must produce
// byte-identical schemas.
func TestPointerAndOptionProduceTheSameSchema(t *testing.T) {
	schemaOf := func(rt reflect.Type) types.SchemaGraph {
		var g graphBuilder
		g.node(compile(rt))
		return g.build()
	}
	ptr := schemaOf(reflect.TypeFor[*string]())
	opt := schemaOf(reflect.TypeFor[Option[string]]())

	if len(ptr.TypeNodes) != len(opt.TypeNodes) {
		t.Fatalf("node counts differ: *string=%d Option[string]=%d", len(ptr.TypeNodes), len(opt.TypeNodes))
	}
	for i := range ptr.TypeNodes {
		if a, b := ptr.TypeNodes[i].Body.Tag(), opt.TypeNodes[i].Body.Tag(); a != b {
			t.Fatalf("node %d differs: *string tag %d, Option[string] tag %d", i, a, b)
		}
	}
	// node 0 is the option (its index is reserved before the inner type is added)
	if ptr.TypeNodes[0].Body.Tag() != types.SchemaTypeBodyOptionType {
		t.Fatal("expected an option-type node at the root")
	}
	if ptr.TypeNodes[1].Body.Tag() != types.SchemaTypeBodyStringType {
		t.Fatal("expected the inner string type")
	}
}

// ---------------------------------------------------------------------------
// recursion
// ---------------------------------------------------------------------------

type Tree struct {
	Label    string
	Children []*Tree
}

// assertNoCycleWithoutRef mirrors the consumer's own validation: it walks the
// type nodes exactly as golem-schema's decoder does, tracking the nodes on the
// current path. Re-entering a node without passing through a ref-type is
// DecodeError::CyclicTypeWithoutRef — a graph the platform rejects outright.
// ref-type deliberately does not recurse into the def body, which is what makes
// it the only valid recursion form.
func assertNoCycleWithoutRef(t *testing.T, g types.SchemaGraph, idx int32, path map[int32]bool) {
	t.Helper()
	if path[idx] {
		t.Fatalf("node %d re-entered without passing through a ref-type: "+
			"this graph would be rejected as CyclicTypeWithoutRef", idx)
	}
	path[idx] = true
	defer delete(path, idx)

	body := g.TypeNodes[idx].Body
	switch body.Tag() {
	case types.SchemaTypeBodyRefType:
		return // a reference terminates the walk
	case types.SchemaTypeBodyRecordType:
		for _, f := range body.RecordType() {
			assertNoCycleWithoutRef(t, g, f.Body, path)
		}
	case types.SchemaTypeBodyVariantType:
		for _, c := range body.VariantType() {
			if c.Payload.IsSome() {
				assertNoCycleWithoutRef(t, g, c.Payload.Some(), path)
			}
		}
	case types.SchemaTypeBodyOptionType:
		assertNoCycleWithoutRef(t, g, body.OptionType(), path)
	case types.SchemaTypeBodyListType:
		assertNoCycleWithoutRef(t, g, body.ListType(), path)
	case types.SchemaTypeBodyFixedListType:
		assertNoCycleWithoutRef(t, g, body.FixedListType().Element, path)
	case types.SchemaTypeBodyMapType:
		spec := body.MapType()
		assertNoCycleWithoutRef(t, g, spec.Key, path)
		assertNoCycleWithoutRef(t, g, spec.Value, path)
	case types.SchemaTypeBodyResultType:
		spec := body.ResultType()
		if spec.Ok.IsSome() {
			assertNoCycleWithoutRef(t, g, spec.Ok.Some(), path)
		}
		if spec.Err.IsSome() {
			assertNoCycleWithoutRef(t, g, spec.Err.Some(), path)
		}
	}
}

func TestRecursiveTypeIsEmittedAsANamedDefNotARawCycle(t *testing.T) {
	c := compile(reflect.TypeFor[Tree]())
	if !c.recursive {
		t.Fatal("Tree should have been detected as recursive at compile time")
	}

	var g graphBuilder
	root := g.node(c)
	graph := g.build()

	// The root of a recursive type is a reference to its def.
	if tag := graph.TypeNodes[root].Body.Tag(); tag != types.SchemaTypeBodyRefType {
		t.Fatalf("root node tag = %d, want ref-type", tag)
	}
	if len(graph.Defs) != 1 {
		t.Fatalf("expected exactly 1 def, got %d", len(graph.Defs))
	}
	def := graph.Defs[0]
	if def.Id == "" {
		t.Fatal("def must carry a stable type-id")
	}
	if !strings.HasSuffix(def.Id, ".Tree") {
		t.Fatalf("type-id %q should end in .Tree", def.Id)
	}
	// The def body is the record itself, not another reference.
	if tag := graph.TypeNodes[def.Body].Body.Tag(); tag != types.SchemaTypeBodyRecordType {
		t.Fatalf("def body tag = %d, want record-type", tag)
	}

	// The whole graph must satisfy the consumer's rule, from the root and from
	// every def body.
	assertNoCycleWithoutRef(t, graph, root, map[int32]bool{})
	for _, d := range graph.Defs {
		assertNoCycleWithoutRef(t, graph, d.Body, map[int32]bool{})
	}

	assertRoundTrip(t, "recursive tree", Tree{
		Label: "root",
		Children: []*Tree{
			{Label: "a", Children: []*Tree{{Label: "a1", Children: []*Tree{}}}},
			{Label: "b", Children: []*Tree{}},
		},
	})
}

// Mutually recursive types: marking one member of each cycle is enough, since
// its ref-type node breaks every path through the cycle.
type nodeA struct {
	Name string
	B    *nodeB
}
type nodeB struct {
	Count int64
	A     []nodeA
}

func TestMutuallyRecursiveTypesBreakTheCycle(t *testing.T) {
	var g graphBuilder
	root := g.node(compile(reflect.TypeFor[nodeA]()))
	graph := g.build()

	assertNoCycleWithoutRef(t, graph, root, map[int32]bool{})
	for _, d := range graph.Defs {
		assertNoCycleWithoutRef(t, graph, d.Body, map[int32]bool{})
	}
	if len(graph.Defs) == 0 {
		t.Fatal("a mutually recursive pair must produce at least one named def")
	}
	assertRoundTrip(t, "mutually recursive", nodeA{
		Name: "a", B: &nodeB{Count: 2, A: []nodeA{{Name: "inner"}}},
	})
}

// Every schema the SDK publishes must satisfy the rule, not just the ones with
// obvious cycles.
func TestPublishedAgentSchemasHaveNoRawCycles(t *testing.T) {
	for _, name := range registryOrder {
		at := buildAgentType(registry[name])
		for _, f := range at.Constructor.InputSchema.Parameters() {
			assertNoCycleWithoutRef(t, at.Schema, f.Schema, map[int32]bool{})
		}
		for _, m := range at.Methods {
			for _, f := range m.InputSchema.Parameters() {
				assertNoCycleWithoutRef(t, at.Schema, f.Schema, map[int32]bool{})
			}
			if m.OutputSchema.Tag() == common.OutputSchemaSingle {
				assertNoCycleWithoutRef(t, at.Schema, m.OutputSchema.Single(), map[int32]bool{})
			}
		}
		for _, d := range at.Schema.Defs {
			assertNoCycleWithoutRef(t, at.Schema, d.Body, map[int32]bool{})
		}
	}
}

func TestDefsAreSortedForDeterminism(t *testing.T) {
	var g graphBuilder
	g.node(compile(reflect.TypeFor[nodeA]()))
	g.node(compile(reflect.TypeFor[Tree]()))
	graph := g.build()

	for i := 1; i < len(graph.Defs); i++ {
		if graph.Defs[i-1].Id > graph.Defs[i].Id {
			t.Fatalf("defs not sorted: %q before %q", graph.Defs[i-1].Id, graph.Defs[i].Id)
		}
	}
	// and the ref nodes still resolve to the right defs after the sort
	for _, n := range graph.TypeNodes {
		if n.Body.Tag() == types.SchemaTypeBodyRefType {
			if int(n.Body.RefType()) >= len(graph.Defs) {
				t.Fatalf("ref-type index %d out of range after sorting", n.Body.RefType())
			}
		}
	}
}

func TestSharedTypeIsEmittedOnce(t *testing.T) {
	type Pair struct {
		Left  Money
		Right Money
	}
	var g graphBuilder
	g.node(compile(reflect.TypeFor[Pair]()))
	graph := g.build()

	records := 0
	for _, n := range graph.TypeNodes {
		if n.Body.Tag() == types.SchemaTypeBodyRecordType {
			records++
		}
	}
	// Pair and Money — Money must not be duplicated for each field.
	if records != 2 {
		t.Fatalf("expected 2 record nodes (Pair, Money), got %d", records)
	}
}

// ---------------------------------------------------------------------------
// determinism and rejection
// ---------------------------------------------------------------------------

func TestMapEncodingIsDeterministic(t *testing.T) {
	// Go randomizes map iteration; these trees land in the oplog and are
	// compared on replay, so encoding must be stable.
	m := map[string]int64{"z": 1, "a": 2, "m": 3, "b": 4, "q": 5}
	c := compile(reflect.TypeFor[map[string]int64]())

	first := encodeWith(c, reflect.ValueOf(m))
	for range 50 {
		again := encodeWith(c, reflect.ValueOf(m))
		if !reflect.DeepEqual(first, again) {
			t.Fatal("map encoding is not deterministic across runs")
		}
	}
}

func mustPanic(t *testing.T, want string, f func()) {
	t.Helper()
	defer func() {
		r := recover()
		if r == nil {
			t.Fatalf("expected a panic mentioning %q", want)
		}
		if msg := fmt.Sprint(r); !strings.Contains(msg, want) {
			t.Fatalf("panic %q does not mention %q", msg, want)
		}
	}()
	f()
}

func TestUnsupportedTypesAreRejectedAtRegistration(t *testing.T) {
	// Registration-time panics, not invocation-time surprises.
	mustPanic(t, "platform-dependent width", func() { compile(reflect.TypeFor[int]()) })
	mustPanic(t, "platform-dependent width", func() { compile(reflect.TypeFor[uint]()) })
	mustPanic(t, "not a primitive", func() { compile(reflect.TypeFor[map[Money]string]()) })
	mustPanic(t, "unsupported type", func() { compile(reflect.TypeFor[chan int]()) })
}

func TestMalformedInputIsAnErrorNotAPanic(t *testing.T) {
	// A string where a record is expected, and a truncated tree.
	c := compile(reflect.TypeFor[Money]())
	tree := types.SchemaValueTree{
		ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodeStringValue("nope")},
		Root:       0,
	}
	out := reflect.New(reflect.TypeFor[Money]()).Elem()
	d := decoder{nodes: tree.ValueNodes}
	if err := c.decode(&d, out, tree.Root); err == nil {
		t.Fatal("expected an error decoding a string into a record")
	}

	empty := decoder{nodes: nil}
	if err := c.decode(&empty, out, 0); err == nil {
		t.Fatal("expected an error for an out-of-range node index")
	}
}

// A nil slice and an empty slice are both an empty list on the wire, so decoding
// normalizes nil to empty. Unlike encoding/json — which emits null for a nil
// slice and [] for an empty one — there is no ambiguity here: []T is ALWAYS
// list-type and never option, so "absent" is simply not representable. Spell the
// optional list *[]T if the distinction matters.
func TestNilSliceDecodesAsEmptySlice(t *testing.T) {
	got := roundTrip(t, Tree{Label: "leaf", Children: nil})
	if got.Children == nil {
		t.Fatal("expected a non-nil empty slice after decoding")
	}
	if len(got.Children) != 0 {
		t.Fatalf("expected an empty slice, got %d element(s)", len(got.Children))
	}
	// A nil map normalizes the same way.
	type withMap struct{ M map[string]int64 }
	if m := roundTrip(t, withMap{M: nil}); m.M == nil || len(m.M) != 0 {
		t.Fatalf("expected a non-nil empty map, got %#v", m.M)
	}
}

// ---------------------------------------------------------------------------
// variants and enums
// ---------------------------------------------------------------------------

// A closed sum type: the unexported marker method means no type outside this
// package can join the variant.
type PaymentMethod interface{ isPaymentMethod() }

type Card struct {
	Number string
	Expiry Status
}
type Cash struct{}
type Transfer struct{ IBAN string }

func (Card) isPaymentMethod()     {}
func (Cash) isPaymentMethod()     {}
func (Transfer) isPaymentMethod() {}

type Status int32

const (
	StatusActive Status = iota
	StatusExpired
	StatusRevoked
)

var _ = DefineEnum[Status]("active", "expired", "revoked")

var _ = DefineVariant[PaymentMethod](
	Case[Card]("card"),
	Case[Cash]("cash"),
	Case[Transfer]("transfer"),
)

func TestRoundTripVariantAndEnum(t *testing.T) {
	assertRoundTrip(t, "enum", StatusExpired)
	assertRoundTrip(t, "variant (card)", PaymentMethod(Card{Number: "4242", Expiry: StatusActive}))
	assertRoundTrip(t, "variant (empty case)", PaymentMethod(Cash{}))
	assertRoundTrip(t, "variant (transfer)", PaymentMethod(Transfer{IBAN: "DE00"}))

	// Variants compose with everything else, which is the point of the codec design.
	assertRoundTrip(t, "option<variant>", Some(PaymentMethod(Cash{})))
	assertRoundTrip(t, "none of variant", None[PaymentMethod]())
	assertRoundTrip(t, "list<variant>", []PaymentMethod{
		Card{Number: "1", Expiry: StatusRevoked}, Cash{}, Transfer{IBAN: "X"},
	})
	assertRoundTrip(t, "result<variant, enum>",
		Ok[PaymentMethod, Status](Card{Number: "9", Expiry: StatusActive}))
	assertRoundTrip(t, "err arm carrying an enum",
		Err[PaymentMethod, Status](StatusRevoked))
	assertRoundTrip(t, "map<string, variant>", map[string]PaymentMethod{
		"a": Cash{}, "b": Transfer{IBAN: "Y"},
	})
}

func TestVariantSchemaNamesCasesInDeclarationOrder(t *testing.T) {
	var g graphBuilder
	root := g.node(compile(reflect.TypeFor[PaymentMethod]()))
	graph := g.build()

	body := graph.TypeNodes[root].Body
	if body.Tag() != types.SchemaTypeBodyVariantType {
		t.Fatalf("expected a variant-type node, got tag %d", body.Tag())
	}
	var names []string
	for _, c := range body.VariantType() {
		names = append(names, c.Name)
	}
	want := []string{"card", "cash", "transfer"}
	if !reflect.DeepEqual(names, want) {
		t.Fatalf("variant cases = %v, want %v", names, want)
	}
}

func TestEnumSchemaCarriesTheDeclaredNames(t *testing.T) {
	var g graphBuilder
	root := g.node(compile(reflect.TypeFor[Status]()))
	graph := g.build()

	body := graph.TypeNodes[root].Body
	if body.Tag() != types.SchemaTypeBodyEnumType {
		t.Fatalf("expected an enum-type node, got tag %d", body.Tag())
	}
	if want := []string{"active", "expired", "revoked"}; !reflect.DeepEqual(body.EnumType(), want) {
		t.Fatalf("enum names = %v, want %v", body.EnumType(), want)
	}
}

func TestVariantAndEnumMisuseIsRejected(t *testing.T) {
	// An interface that was never declared as a variant.
	type Unregistered interface{ marker() }
	mustPanic(t, "not a registered variant", func() { compile(reflect.TypeFor[Unregistered]()) })

	// A value outside the declared enum range must not be silently truncated.
	c := compile(reflect.TypeFor[Status]())
	mustPanic(t, "outside the declared enum range", func() {
		encodeWith(c, reflect.ValueOf(Status(99)))
	})

	// A nil interface holds no case.
	vc := compile(reflect.TypeFor[PaymentMethod]())
	mustPanic(t, "must hold one of its cases", func() {
		encodeWith(vc, reflect.ValueOf(&[]PaymentMethod{nil}[0]).Elem())
	})

	// Declaration-time validation.
	mustPanic(t, "requires an interface type", func() { DefineVariant[Money]() })
	mustPanic(t, "requires a named integer type", func() { DefineEnum[string]("a") })
}

// A decoded variant must be usable through its interface, not just structurally
// equal — the decoder sets a concrete type into the interface slot.
func TestDecodedVariantSatisfiesItsInterface(t *testing.T) {
	got := roundTrip(t, PaymentMethod(Transfer{IBAN: "NL01"}))
	tr, ok := got.(Transfer)
	if !ok {
		t.Fatalf("decoded value is %T, want Transfer", got)
	}
	if tr.IBAN != "NL01" {
		t.Fatalf("IBAN = %q", tr.IBAN)
	}
}

// ---------------------------------------------------------------------------
// markers, time and secrets
// ---------------------------------------------------------------------------

func TestRoundTripMarkersAndTimeTypes(t *testing.T) {
	assertRoundTrip(t, "char", Char('é'))
	assertRoundTrip(t, "url", URL("https://golem.cloud/agents"))
	assertRoundTrip(t, "duration", 90*time.Second)
	assertRoundTrip(t, "datetime", time.Unix(1_700_000_000, 123_456_789).UTC())

	// and they compose like everything else
	assertRoundTrip(t, "option<datetime>", Some(time.Unix(42, 0).UTC()))
	assertRoundTrip(t, "list<url>", []URL{"https://a.example", "https://b.example"})
}

func TestMarkersLowerToTheirOwnWitTypes(t *testing.T) {
	// Char is an int32 and URL is a string underneath, so without recognition by
	// named type they would silently lower to s32 and string.
	for _, tc := range []struct {
		name string
		rt   reflect.Type
		want uint8
	}{
		{"Char", reflect.TypeFor[Char](), types.SchemaTypeBodyCharType},
		{"URL", reflect.TypeFor[URL](), types.SchemaTypeBodyUrlType},
		{"time.Time", reflect.TypeFor[time.Time](), types.SchemaTypeBodyDatetimeType},
		{"time.Duration", reflect.TypeFor[time.Duration](), types.SchemaTypeBodyDurationType},
	} {
		var g graphBuilder
		root := g.node(compile(tc.rt))
		if got := g.build().TypeNodes[root].Body.Tag(); got != tc.want {
			t.Errorf("%s lowered to tag %d, want %d", tc.name, got, tc.want)
		}
	}
}

func TestSecretRoundTripsAndStaysOutOfLogs(t *testing.T) {
	got := roundTrip(t, NewSecret("hunter2"))
	if got.Reveal() != "hunter2" {
		t.Fatalf("revealed %q", got.Reveal())
	}
	// The whole point: formatting must not leak the payload.
	for _, s := range []string{
		fmt.Sprintf("%v", got), fmt.Sprintf("%s", got), fmt.Sprintf("%#v", got),
	} {
		if strings.Contains(s, "hunter2") {
			t.Fatalf("secret leaked through formatting: %s", s)
		}
	}

	// The schema marks it secret, while the value stays the revealed type.
	var g graphBuilder
	root := g.node(compile(reflect.TypeFor[Secret[string]]()))
	if tag := g.build().TypeNodes[root].Body.Tag(); tag != types.SchemaTypeBodySecretType {
		t.Fatalf("Secret lowered to tag %d, want secret-type", tag)
	}
	assertRoundTrip(t, "secret in a record", struct {
		Name  string
		Token Secret[string]
	}{Name: "svc", Token: NewSecret("abc")})
}

// Containers must never be modelled as optional: absence is only expressible
// through a pointer or Option[T], so there is no nil-vs-empty ambiguity.
func TestNilContainersAreNeverOptional(t *testing.T) {
	tagOf := func(rt reflect.Type) uint8 {
		var g graphBuilder
		root := g.node(compile(rt))
		return g.build().TypeNodes[root].Body.Tag()
	}
	for _, tc := range []struct {
		name string
		rt   reflect.Type
		want uint8
	}{
		{"[]string", reflect.TypeFor[[]string](), types.SchemaTypeBodyListType},
		{"[]byte", reflect.TypeFor[[]byte](), types.SchemaTypeBodyListType},
		{"map[string]int64", reflect.TypeFor[map[string]int64](), types.SchemaTypeBodyMapType},
		{"[2]int64", reflect.TypeFor[[2]int64](), types.SchemaTypeBodyFixedListType},
		{"*[]string", reflect.TypeFor[*[]string](), types.SchemaTypeBodyOptionType},
	} {
		if got := tagOf(tc.rt); got != tc.want {
			t.Errorf("%s lowered to tag %d, want %d", tc.name, got, tc.want)
		}
	}

	// A nil slice encodes as an EMPTY LIST, never as none.
	var nilSlice []string
	tree := encodeWith(compile(reflect.TypeFor[[]string]()), reflect.ValueOf(&nilSlice).Elem())
	if n := tree.ValueNodes[tree.Root]; n.Tag() != types.SchemaValueNodeListValue || len(n.ListValue()) != 0 {
		t.Fatalf("nil slice encoded as tag %d", n.Tag())
	}
	var nilMap map[string]int64
	mt := encodeWith(compile(reflect.TypeFor[map[string]int64]()), reflect.ValueOf(&nilMap).Elem())
	if n := mt.ValueNodes[mt.Root]; n.Tag() != types.SchemaValueNodeMapValue || len(n.MapValue()) != 0 {
		t.Fatalf("nil map encoded as tag %d", n.Tag())
	}

	// Only *[]T distinguishes absent from empty.
	pc := compile(reflect.TypeFor[*[]string]())
	var absent *[]string
	if tr := encodeWith(pc, reflect.ValueOf(&absent).Elem()); !tr.ValueNodes[tr.Root].OptionValue().IsNone() {
		t.Fatal("a nil *[]string must encode as none")
	}
	empty := []string{}
	present := &empty
	if tr := encodeWith(pc, reflect.ValueOf(&present).Elem()); tr.ValueNodes[tr.Root].OptionValue().IsNone() {
		t.Fatal("a pointer to an empty slice must encode as some(empty list)")
	}
}
