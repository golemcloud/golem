package golem

import (
	"errors"
	"reflect"
	"strings"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// What is testable natively, and why the rest is not.
//
// `empty.s` lets a generated package COMPILE for the host, but the linker still
// needs a definition for any `//go:wasmimport` symbol that host-arch code
// actually references. So anything reaching a host import — Call, Trigger,
// Schedule, CallAsync, Future.Get/Cancel, ClientFor — can only run under wasm,
// and is covered by the component build plus the Phase 8 end-to-end tests.
//
// Everything the SDK itself is responsible for is pure and covered here:
// encoding arguments, decoding results, and mapping errors. That is where the
// bugs would be; the wrappers are a handful of straight-line calls.

type tPayId struct{ Merchant string }
type tPayState struct{ charged int64 }
type tChargeIn struct {
	AmountCents int64
	Note        *string
}

var tPayment = DefineAgent[tPayId, tPayState](
	Spec{Name: "TestPayment", Mode: Durable},
	func(id tPayId) *tPayState { return &tPayState{} },
)

var tCharge = DefineMethod[tPayId, tChargeIn, Money]("charge")

func init() {
	Implement(tPayment, tCharge, func(ctx *Context[tPayState], in tChargeIn) Money {
		ctx.State.charged += in.AmountCents
		return Money{Amount: ctx.State.charged, Currency: "EUR"}
	})
}

// The caller encodes arguments with the same codecs the callee decodes with,
// because both are derived from the same Go types. This is the symmetry claim
// the design rests on, and it is checkable without a host.
func TestCallerEncodingMatchesCalleeDecoding(t *testing.T) {
	note := "tip"
	in := tChargeIn{AmountCents: 1250, Note: &note}

	tree, err := tCharge.encodeInput(in)
	if err != nil {
		t.Fatalf("encodeInput: %v", err)
	}

	// Decode it the way the callee's invoke path does.
	fields := defs.structFields(reflect.TypeFor[tChargeIn]())
	dst := reflect.New(reflect.TypeFor[tChargeIn]()).Elem()
	if err := decodeParams(tree, fields, dst); err != nil {
		t.Fatalf("callee could not decode the caller's arguments: %v", err)
	}
	if got := dst.Interface().(tChargeIn); !reflect.DeepEqual(got, in) {
		t.Fatalf("round trip changed the arguments\n got: %#v\nwant: %#v", got, in)
	}
}

// The encoded argument list must agree with the schema the callee publishes.
func TestCallArgumentsAgreeWithThePublishedSchema(t *testing.T) {
	tree, err := tCharge.encodeInput(tChargeIn{AmountCents: 7})
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

// checkAgreement2 adapts the Phase 3 helper, which returns an error.
func checkAgreement2(t *testing.T, g types.SchemaGraph, s int32, tree types.SchemaValueTree, v int32, path string) {
	t.Helper()
	if err := checkAgreement(g, s, tree, v, path); err != nil {
		t.Fatalf("argument %q disagrees with its declared type: %v", path, err)
	}
}

// The constructor tree a client builds must be decodable as the target's
// constructor parameters — otherwise the agent id would address nothing.
func TestClientConstructorTreeMatchesTheTargetConstructor(t *testing.T) {
	e := defs.agents["TestPayment"]
	id := tPayId{Merchant: "acme"}
	ctor := encodeParams(e.idFields, reflect.ValueOf(&id).Elem())

	dst := reflect.New(reflect.TypeFor[tPayId]()).Elem()
	if err := decodeParams(ctor, e.idFields, dst); err != nil {
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
	if rce.Kind != RemoteProtocol || rce.Method != "charge" || rce.Target != "agent-1" {
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
		want RemoteErrorKind
	}{
		{"protocol", host.MakeRpcErrorProtocolError("bad frame"), RemoteProtocol},
		{"denied", host.MakeRpcErrorDenied("nope"), RemoteDenied},
		{"not found", host.MakeRpcErrorNotFound("no such agent"), RemoteNotFound},
		{"internal", host.MakeRpcErrorRemoteInternalError("boom"), RemoteInternal},
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

// A remote domain error must stay inspectable rather than collapsing to a string.
func TestRemoteAgentErrorKeepsItsCause(t *testing.T) {
	remote := common.MakeAgentErrorInvalidMethod("no such method: charge")
	err := rpcErrorToGo("agent-1", "charge", host.MakeRpcErrorRemoteAgentError(remote))

	var rce *RemoteCallError
	if !errors.As(err, &rce) {
		t.Fatalf("expected *RemoteCallError, got %T", err)
	}
	if rce.Kind != RemoteAgent {
		t.Fatalf("kind = %v, want RemoteAgent", rce.Kind)
	}
	cause := errors.Unwrap(rce)
	if cause == nil {
		t.Fatal("a remote agent-error must be preserved as the cause")
	}
	if !strings.Contains(cause.Error(), "no such method") {
		t.Fatalf("cause lost the remote detail: %v", cause)
	}
}

// Note: Call/Trigger/CallAsync guard against a zero Client, and Future.Get
// guards against reuse after its owned handle is dropped. Both are straight-line
// nil checks that cannot be exercised natively (referencing them pulls in the
// wasmimport symbols), so they are covered by the wasip1 build and Phase 8.
