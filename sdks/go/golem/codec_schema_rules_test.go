// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"math/big"
	"net/url"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/core/values"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

func rootBody(t *testing.T, rt reflect.Type) types.SchemaTypeBody {
	t.Helper()
	c := defs.Compile(rt)
	if c.Invalid != "" {
		t.Fatalf("%s: %s", rt, c.Invalid)
	}
	g := engine.GraphBuilder{E: defs.Engine}
	root := g.Node(c)
	return g.Build().TypeNodes[root].Body
}

// graphInvalid is what building the schema graph of rt reports as unencodable,
// anywhere inside it.
func graphInvalid(rt reflect.Type) string {
	g := engine.GraphBuilder{E: defs.Engine}
	g.Node(defs.Compile(rt))
	var out []string
	for _, why := range g.Invalids {
		out = append(out, why)
	}
	return strings.Join(out, "; ")
}

func encodeValue[T any](in T) types.SchemaValueTree {
	return engine.EncodeWith(defs.Compile(reflect.TypeFor[T]()), reflect.ValueOf(&in).Elem())
}

func decodeValue[T any](tree types.SchemaValueTree) (T, error) {
	var out T
	d := engine.Decoder{Nodes: tree.ValueNodes}
	err := defs.Compile(reflect.TypeFor[T]()).Decode(&d, reflect.ValueOf(&out).Elem(), tree.Root)
	return out, err
}

// Unit is the empty tuple, as Rust's () is, so a callee's tuple<> has a Go
// spelling.
func TestUnitIsTheEmptyTuple(t *testing.T) {
	body := rootBody(t, reflect.TypeFor[Unit]())
	if body.Tag() != types.SchemaTypeBodyTupleType || len(body.TupleType()) != 0 {
		t.Fatalf("Unit lowers to tag %d, want the empty tuple", body.Tag())
	}
	assertRoundTrip(t, "unit", Unit{})
	if reflect.TypeFor[Unit]() != reflect.TypeFor[values.Unit]() {
		t.Fatal("golem.Unit is not values.Unit")
	}
}

// A Unit arm of a Result is an arm without a payload, which is what result<_, E>
// declares and what the host checks a value of it against.
func TestAUnitResultArmCarriesNoPayload(t *testing.T) {
	body := rootBody(t, reflect.TypeFor[Result[Unit, string]]())
	if spec := body.ResultType(); spec.Ok.IsSome() || spec.Err.IsNone() {
		t.Fatalf("Result[Unit, string] publishes ok=%v err=%v, want no ok type", spec.Ok.IsSome(), spec.Err.IsSome())
	}
	body = rootBody(t, reflect.TypeFor[Result[string, Unit]]())
	if spec := body.ResultType(); spec.Ok.IsNone() || spec.Err.IsSome() {
		t.Fatalf("Result[string, Unit] publishes ok=%v err=%v, want no err type", spec.Ok.IsSome(), spec.Err.IsSome())
	}

	tree := encodeValue(Ok[Unit, string](Unit{}))
	if p := tree.ValueNodes[tree.Root].ResultValue(); p.Tag() != types.ResultValuePayloadOkValue || p.OkValue().IsSome() {
		t.Fatal("an ok Unit arm travelled with a payload")
	}
	tree = encodeValue(Err[string](Unit{}))
	if p := tree.ValueNodes[tree.Root].ResultValue(); p.Tag() != types.ResultValuePayloadErrValue || p.ErrValue().IsSome() {
		t.Fatal("an err Unit arm travelled with a payload")
	}
	assertRoundTrip(t, "ok unit", Ok[Unit, string](Unit{}))
	assertRoundTrip(t, "err unit", Err[string](Unit{}))
	assertRoundTrip(t, "err beside unit", Err[Unit]("boom"))

	withPayload := types.SchemaValueTree{
		ValueNodes: []types.SchemaValueNode{
			types.MakeSchemaValueNodeTupleValue(nil),
			types.MakeSchemaValueNodeResultValue(types.MakeResultValuePayloadOkValue(witTypes.Some[int32](0))),
		},
		Root: 1,
	}
	if _, err := decodeValue[Result[Unit, string]](withPayload); err == nil || !strings.Contains(err.Error(), "declares no value") {
		t.Fatalf("a payload on a unit arm decoded: %v", err)
	}
}

// An empty record is still a record when it is a Result arm: only Unit is the
// arm without a payload.
func TestAnEmptyStructResultArmIsARecord(t *testing.T) {
	type Nothing struct{}
	body := rootBody(t, reflect.TypeFor[Result[Nothing, string]]())
	if body.ResultType().Ok.IsNone() {
		t.Fatal("Result[Nothing, string] lost its ok type")
	}
}

// A map key is accepted exactly when the host accepts its schema: bool, an
// integer, a float, char or string. A Go type of a primitive kind that lowers
// to another schema type is refused.
func TestMapKeysFollowTheSchemaRule(t *testing.T) {
	for _, rt := range []reflect.Type{
		reflect.TypeFor[map[time.Duration]string](),
		reflect.TypeFor[map[values.Text]string](),
		reflect.TypeFor[map[values.URL]string](),
		reflect.TypeFor[map[values.Path]string](),
		reflect.TypeFor[map[Status]string](),
	} {
		mustInvalidCompile(t, "map key", rt)
	}
	for _, rt := range []reflect.Type{
		reflect.TypeFor[map[bool]string](),
		reflect.TypeFor[map[int8]string](),
		reflect.TypeFor[map[uint64]string](),
		reflect.TypeFor[map[float32]string](),
		reflect.TypeFor[map[values.Char]string](),
		reflect.TypeFor[map[string]string](),
	} {
		if c := defs.Compile(rt); c.Invalid != "" {
			t.Errorf("%s: %s", rt, c.Invalid)
		}
	}
}

type opaqueTime time.Time

// A struct whose fields are all unexported has nothing a record could carry, so
// it would travel as record {} and arrive as a zero value. It is refused.
func TestAStructWithOnlyUnexportedFieldsIsRefused(t *testing.T) {
	mustInvalidCompile(t, "no exported fields", reflect.TypeFor[big.Int]())
	mustInvalidCompile(t, "no exported fields", reflect.TypeFor[opaqueTime]())
	mustInvalidCompile(t, "no exported fields", reflect.TypeFor[url.Userinfo]())
	if why := graphInvalid(reflect.TypeFor[url.URL]()); !strings.Contains(why, "url.Userinfo has no exported fields") {
		t.Fatalf("url.URL compiled although its User field cannot travel: %q", why)
	}
}

// A Future is a handle on a call in flight, not data.
func TestAFutureIsNotSendable(t *testing.T) {
	mustInvalidCompile(t, "golem.Future", reflect.TypeFor[Future[int64]]())
	if why := graphInvalid(reflect.TypeFor[*Future[int64]]()); !strings.Contains(why, "golem.Future") {
		t.Fatalf("*Future compiled: %q", why)
	}
}

// A map value that names one key twice is accepted, and the later entry wins,
// as the host's validation and the Rust conversion do.
func TestADuplicateMapKeyKeepsTheLaterEntry(t *testing.T) {
	tree := types.SchemaValueTree{
		ValueNodes: []types.SchemaValueNode{
			types.MakeSchemaValueNodeStringValue("k"),
			types.MakeSchemaValueNodeS64Value(1),
			types.MakeSchemaValueNodeS64Value(2),
			types.MakeSchemaValueNodeMapValue([]types.MapEntry{{Key: 0, Value: 1}, {Key: 0, Value: 2}}),
		},
		Root: 3,
	}
	got, err := decodeValue[map[string]int64](tree)
	if err != nil || !reflect.DeepEqual(got, map[string]int64{"k": 2}) {
		t.Fatalf("got %v, %v", got, err)
	}
}

// A quantity's unit is checked against its marker both ways.
func TestAQuantityUnitIsCheckedBothWays(t *testing.T) {
	mustPanic(t, `unit "lb"`, func() { encodeValue(Quantity[Weight]{Mantissa: 1, Unit: "lb"}) })
	mustPanic(t, `unit "kB"`, func() { encodeValue(Quantity[Bytes]{Mantissa: 1, Unit: "kB"}) })

	tree := types.SchemaValueTree{ValueNodes: []types.SchemaValueNode{
		types.MakeSchemaValueNodeQuantityValueNode(types.QuantityValue{Mantissa: 1, Unit: "lb"}),
	}}
	if _, err := decodeValue[Quantity[Weight]](tree); err == nil || !strings.Contains(err.Error(), `unit "lb"`) {
		t.Fatalf("a disallowed unit decoded: %v", err)
	}
	tree.ValueNodes[0] = types.MakeSchemaValueNodeQuantityValueNode(types.QuantityValue{Mantissa: 1, Unit: "kB"})
	if _, err := decodeValue[Quantity[Bytes]](tree); err == nil {
		t.Fatal("a unit other than the base unit decoded for a marker without suffixes")
	}
	assertRoundTrip(t, "allowed suffix", Quantity[Weight]{Mantissa: 1, Unit: "g"})
}

// An argument that is not valid UTF-8 is an error from the call, not a panic.
func TestInvalidUTF8InACallArgumentIsAnError(t *testing.T) {
	note := "ok\xffno"
	_, err := tCharge.encodeInput(tChargeIn{AmountCents: 1, Note: &note})
	if err == nil || !strings.Contains(err.Error(), "not valid UTF-8") {
		t.Fatalf("got %v", err)
	}
}
