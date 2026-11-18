package golem

import (
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// The Go SDK's tests are layered (see the go-sdk-testing-strategy notes):
//
//   - Pure logic is unit-tested here, natively, under `go test`: argument
//     encoding, result decoding, error mapping, and the small value projections.
//     None of these functions reference a `//go:wasmimport` symbol, so they link
//     and run on the host. We push as much logic as possible into this layer.
//   - The wasi-touching wrappers — Call, Trigger, Schedule, CallAsync,
//     Future.Get/Cancel, ClientFor — cannot run natively: calling one references
//     a wasmimport symbol the host linker can't resolve (`empty.s` lets the
//     generated package compile, but not link a referenced import). These are
//     covered by the CLI-driven and runtime (worker-executor) integration layers
//     instead, which build the component to wasm and run it.

type tPayId struct{ Merchant string }
type tPayState struct{ charged int64 }
type tChargeIn struct {
	AmountCents int64
	Note        *string
}

var tPayment = DefineAgent[tPayId](
	Spec{Name: "TestPayment", Mode: Durable},
)

var tCharge = tPayment.Method[tChargeIn, Money]("charge")

func init() {
	p := tPayment.Implement(func(id tPayId) *tPayState { return &tPayState{} })
	p.Handle(tCharge, func(ctx *Context[tPayState], in tChargeIn) Money {
		ctx.State.charged += in.AmountCents
		return Money{Amount: ctx.State.charged, Currency: "EUR"}
	})
}

// TestCallerEncodingMatchesCalleeDecoding — The caller encodes arguments with the same codecs the callee decodes with,
// because both are derived from the same Go types. This is the symmetry claim
// the design rests on, and it is checkable without a host.
func TestCallerEncodingMatchesCalleeDecoding(t *testing.T) {
	note := "tip"
	in := tChargeIn{AmountCents: 1250, Note: &note}

	tree, err := tCharge.encodeInput("target", in)
	if err != nil {
		t.Fatalf("encodeInput: %v", err)
	}

	// Decode it the way the callee's invoke path does.
	fields := defs.StructFields(reflect.TypeFor[tChargeIn]())
	dst := reflect.New(reflect.TypeFor[tChargeIn]()).Elem()
	if err := decodeParams(tree, fields, dst, nil); err != nil {
		t.Fatalf("callee could not decode the caller's arguments: %v", err)
	}
	if got := dst.Interface().(tChargeIn); !reflect.DeepEqual(got, in) {
		t.Fatalf("round trip changed the arguments\n got: %#v\nwant: %#v", got, in)
	}
}

// TestCallArgumentsAgreeWithThePublishedSchema — The encoded argument list must agree with the schema the callee publishes.
func TestCallArgumentsAgreeWithThePublishedSchema(t *testing.T) {
	tree, err := tCharge.encodeInput("target", tChargeIn{AmountCents: 7})
	if err != nil {
		t.Fatalf("encodeInput: %v", err)
	}
	at, _ := defs.buildAgentType(defs.agents["TestPayment"])

	var charge *common.AgentMethod
	for i := range at.Methods {
		if at.Methods[i].Name == "charge" {
			charge = &at.Methods[i]
		}
	}
	if charge == nil {
		t.Fatal("charge missing from the published agent type")
	}

	params := charge.InputSchema.Parameters()
	root := tree.ValueNodes[tree.Root]
	if root.Tag() != types.SchemaValueNodeRecordValue {
		t.Fatalf("argument list root tag = %d, want record", root.Tag())
	}
	idxs := root.RecordValue()
	if len(idxs) != len(params) {
		t.Fatalf("encoded %d argument(s), schema declares %d", len(idxs), len(params))
	}
	// Each argument must match its declared parameter type.
	for i, p := range params {
		checkAgreement2(t, at.Schema, p.Schema, tree, idxs[i], p.Name)
	}
}

type tScaleID struct{ Name string }
type tScaleState struct{}
type tWeighIn struct{ Load Quantity[Weight] }

var tScale = DefineAgent[tScaleID](Spec{Name: "TestScale", Mode: Durable})

var tWeigh = tScale.Method[tWeighIn, Unit]("weigh")

func init() {
	s := tScale.Implement(func(tScaleID) *tScaleState { return &tScaleState{} })
	s.Handle(tWeigh, func(*Context[tScaleState], tWeighIn) Unit { return Unit{} })
}

// An input the callee's schema does not accept is a protocol error of the call,
// returned before anything is sent, not a panic that traps the caller.
func TestAnInputThatDoesNotEncodeIsARemoteProtocolError(t *testing.T) {
	_, err := tWeigh.encodeInput("scale-1", tWeighIn{Load: Quantity[Weight]{Mantissa: 1, Unit: "lb"}})
	var rce *RemoteCallError
	if !errors.As(err, &rce) {
		t.Fatalf("got %v, want a *RemoteCallError", err)
	}
	if rce.Kind != RemoteProtocolError || rce.Target != "scale-1" || rce.Method != "weigh" || !strings.Contains(rce.Message, `unit "lb"`) {
		t.Fatalf("got %+v", rce)
	}
	if _, err := tWeigh.encodeInput("scale-1", tWeighIn{Load: Quantity[Weight]{Mantissa: 1, Unit: "g"}}); err != nil {
		t.Fatalf("an allowed unit was refused: %v", err)
	}
}

// An id the target's constructor does not accept is an error, not a panic.
func TestAnIdThatDoesNotEncodeIsAnError(t *testing.T) {
	type scaleID struct{ Load Quantity[Weight] }
	fields := defs.StructFields(reflect.TypeFor[scaleID]())
	id := scaleID{Load: Quantity[Weight]{Mantissa: 1, Unit: "lb"}}
	if _, err := tryEncodeParams(fields, reflect.ValueOf(&id).Elem()); err == nil || !strings.Contains(err.Error(), `unit "lb"`) {
		t.Fatalf("got %v", err)
	}
}

// checkAgreement2 fails the test when checkAgreement, which returns an error, finds a disagreement.
func checkAgreement2(t *testing.T, g types.SchemaGraph, s int32, tree types.SchemaValueTree, v int32, path string) {
	t.Helper()
	if err := checkAgreement(g, s, tree, v, path); err != nil {
		t.Fatalf("argument %q disagrees with its declared type: %v", path, err)
	}
}

// TestClientConstructorTreeMatchesTheTargetConstructor — The constructor tree a client builds must be decodable as the target's
// constructor parameters — otherwise the agent id would address nothing.
func TestClientConstructorTreeMatchesTheTargetConstructor(t *testing.T) {
	e := defs.agents["TestPayment"]
	id := tPayId{Merchant: "acme"}
	ctor := encodeParams(e.idFields, reflect.ValueOf(&id).Elem())

	dst := reflect.New(reflect.TypeFor[tPayId]()).Elem()
	if err := decodeParams(ctor, e.idFields, dst, nil); err != nil {
		t.Fatalf("target cannot decode the client's constructor tree: %v", err)
	}
	if got := dst.Interface().(tPayId); got != id {
		t.Fatalf("constructor round trip = %#v, want %#v", got, id)
	}
}

func TestDecodeOutputRejectsAMismatchedRemote(t *testing.T) {
	// A remote claiming to return Money but sending a bare string.
	bad := types.SchemaValueTree{
		ValueNodes: []types.SchemaValueNode{types.MakeSchemaValueNodeStringValue("nope")},
		Root:       0,
	}
	_, err := decodeOutput[Money]("agent-1", "charge", witTypes.Some(bad))
	if err == nil {
		t.Fatal("expected an error decoding a string as Money")
	}
	var rce *RemoteCallError
	if !errors.As(err, &rce) {
		t.Fatalf("expected a *RemoteCallError, got %T", err)
	}
	if rce.Kind != RemoteProtocolError || rce.Method != "charge" || rce.Target != "agent-1" {
		t.Fatalf("unexpected error detail: %+v", rce)
	}

	// A non-unit output with no value at all.
	if _, err := decodeOutput[Money]("agent-1", "charge", witTypes.None[types.SchemaValueTree]()); err == nil {
		t.Fatal("expected an error for a missing non-unit output")
	}

	// A unit output legitimately carries nothing.
	if _, err := decodeOutput[Unit]("agent-1", "reset", witTypes.None[types.SchemaValueTree]()); err != nil {
		t.Fatalf("unit output should accept no value: %v", err)
	}
}

func TestRpcErrorsMapToDistinguishableKinds(t *testing.T) {
	for _, tc := range []struct {
		name string
		in   host.RpcError
		want RemoteCallErrorKind
	}{
		{"protocol", host.MakeRpcErrorProtocolError("bad frame"), RemoteProtocolError},
		{"denied", host.MakeRpcErrorDenied("nope"), RemoteDenied},
		{"not found", host.MakeRpcErrorNotFound("no such agent"), RemoteNotFound},
		{"internal", host.MakeRpcErrorRemoteInternalError("boom"), RemoteInternalError},
	} {
		err := rpcErrorToGo("agent-1", "charge", tc.in)
		var rce *RemoteCallError
		if !errors.As(err, &rce) {
			t.Fatalf("%s: expected *RemoteCallError, got %T", tc.name, err)
		}
		if rce.Kind != tc.want {
			t.Errorf("%s: kind = %v, want %v", tc.name, rce.Kind, tc.want)
		}
		if !strings.Contains(err.Error(), "agent-1") || !strings.Contains(err.Error(), "charge") {
			t.Errorf("%s: message should name the target and method: %s", tc.name, err)
		}
	}
}

// TestRemoteAgentErrorKeepsItsCause — A remote domain error must stay inspectable rather than collapsing to a string.
func TestRemoteAgentErrorKeepsItsCause(t *testing.T) {
	remote := common.MakeAgentErrorInvalidMethod("no such method: charge")
	err := rpcErrorToGo("agent-1", "charge", host.MakeRpcErrorRemoteAgentError(remote))

	var rce *RemoteCallError
	if !errors.As(err, &rce) {
		t.Fatalf("expected *RemoteCallError, got %T", err)
	}
	if rce.Kind != RemoteAgentError {
		t.Fatalf("kind = %v, want RemoteAgentError", rce.Kind)
	}
	cause := errors.Unwrap(rce)
	if cause == nil {
		t.Fatal("a remote agent-error must be preserved as the cause")
	}
	if !strings.Contains(cause.Error(), "no such method") {
		t.Fatalf("cause lost the remote detail: %v", cause)
	}
}

// TestInvocationMetadataFromProjects — invocationMetadataFrom projects the
// host's invocation metadata onto the SDK's public InvocationMetadata: the
// invocation ID takes the metadata's agent ID and the host ID's UUID. Pure, so
// it is pinned here rather than left to integration.
func TestInvocationMetadataFromProjects(t *testing.T) {
	got := invocationMetadataFrom(testHostInvocationMetadata)
	if got != testInvocationMetadata {
		t.Fatalf("invocationMetadataFrom = %+v, want %+v", got, testInvocationMetadata)
	}
}

// TestAsyncInvocationMetadataIsAbsentForARejectedCall — the host reports no
// metadata for a call it rejected before it started; the future then has no
// identity rather than a zero one.
func TestAsyncInvocationMetadataIsAbsentForARejectedCall(t *testing.T) {
	if m := asyncInvocationMetadata(witTypes.None[host.InvocationMetadata]()); m != nil {
		t.Fatalf("asyncInvocationMetadata(none) = %+v, want nil", *m)
	}
	m := asyncInvocationMetadata(witTypes.Some(testHostInvocationMetadata))
	if m == nil || *m != testInvocationMetadata {
		t.Fatalf("asyncInvocationMetadata(some) = %v, want %+v", m, testInvocationMetadata)
	}
}

var testInvocationUUID = UUID{0x0b, 0x9f, 0x3d, 0x9e, 0x5f, 0x43, 0x4c, 0x3a, 0x9e, 0x7c, 0x0d, 0x6f, 0x1f, 0x2a, 0x3b, 0x4c}

var testHostInvocationMetadata = host.InvocationMetadata{
	AgentId: "agent-7",
	InvocationId: types.InvocationId{
		Owner: types.MakeInvocationOwnerTool("ignored"),
		Id:    uuidToWit(testInvocationUUID),
	},
}

var testInvocationMetadata = InvocationMetadata{
	AgentID:      "agent-7",
	InvocationID: InvocationID{AgentID: "agent-7", UUID: testInvocationUUID},
}

// TestInvocationIDTextFormRoundTrips — the text form is <agent-id>.<uuid>, and
// parsing splits off the trailing UUID, so an agent ID with ".", "/" and quotes
// comes back unchanged.
func TestInvocationIDTextFormRoundTrips(t *testing.T) {
	id := InvocationID{AgentID: `shop.v2/cart("a.b", "x/y")`, UUID: testInvocationUUID}
	const text = `shop.v2/cart("a.b", "x/y").0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c`
	if got := id.String(); got != text {
		t.Fatalf("String() = %q, want %q", got, text)
	}
	parsed, err := ParseInvocationID(text)
	if err != nil {
		t.Fatalf("ParseInvocationID(%q): %v", text, err)
	}
	if parsed != id {
		t.Fatalf("ParseInvocationID(%q) = %+v, want %+v", text, parsed, id)
	}
}

// TestParseInvocationIDAcceptsUpperCaseHex — the UUID part may use upper-case
// hex; the parsed ID prints the canonical lower-case form.
func TestParseInvocationIDAcceptsUpperCaseHex(t *testing.T) {
	const text = `shop.v2/cart("a.b").0B9F3D9E-5F43-4C3A-9E7C-0D6F1F2A3B4C`
	parsed, err := ParseInvocationID(text)
	if err != nil {
		t.Fatalf("ParseInvocationID(%q): %v", text, err)
	}
	want := InvocationID{AgentID: `shop.v2/cart("a.b")`, UUID: testInvocationUUID}
	if parsed != want {
		t.Fatalf("ParseInvocationID(%q) = %+v, want %+v", text, parsed, want)
	}
	const canonical = `shop.v2/cart("a.b").0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c`
	if got := parsed.String(); got != canonical {
		t.Fatalf("String() = %q, want %q", got, canonical)
	}
}

// TestParseInvocationIDRejectsMalformedText — a missing ".", a bad or
// non-hyphenated UUID and an empty agent ID are errors.
func TestParseInvocationIDRejectsMalformedText(t *testing.T) {
	for _, text := range []string{
		"",
		"0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c",
		".0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c",
		"agent-0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c",
		"agent.0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4",
		"agent.0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4g",
		"agent.0b9f3d9e+5f43-4c3a-9e7c-0d6f1f2a3b4c",
		"agent.0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c.",
		"agent.0b9f3d9e5f434c3a9e7c0d6f1f2a3b4c",
		"agent.abc.0b9f3d9e5f434c3a9e7c0d6f1f2a3b4c",
	} {
		if got, err := ParseInvocationID(text); err == nil {
			t.Errorf("ParseInvocationID(%q) = %+v, want an error", text, got)
		}
	}
}

// TestInstantFromConvertsTime — instantFrom converts a Go time into the wasi clock instant used for scheduling.
// The interesting cases are sub-second nanoseconds and pre-epoch times, where the
// seconds go negative while the nanoseconds stay in [0, 1e9).
func TestInstantFromConvertsTime(t *testing.T) {
	normal := instantFrom(time.Unix(1_700_000_000, 250_000_000))
	if normal.Seconds != 1_700_000_000 || normal.Nanoseconds != 250_000_000 {
		t.Fatalf("instantFrom(normal) = {%d, %d}, want {1700000000, 250000000}",
			normal.Seconds, normal.Nanoseconds)
	}

	preEpoch := instantFrom(time.Unix(-2, 500_000_000))
	if preEpoch.Seconds != -2 || preEpoch.Nanoseconds != 500_000_000 {
		t.Fatalf("instantFrom(pre-epoch) = {%d, %d}, want {-2, 500000000}",
			preEpoch.Seconds, preEpoch.Nanoseconds)
	}
}

// Note: the zero-Client guards on Call/Trigger/CallAsync and the consumed-handle
// guard on Future.Get are straight-line nil checks, but they cannot be exercised
// natively — referencing the wrapper pulls in its wasmimport symbols. They are
// covered by the integration layers.
