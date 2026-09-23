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

package schema

import (
	"strings"
	"testing"

	witTypes "go.bytecodealliance.org/pkg/wit/types"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// graphBuilder assembles the schema graphs the tests validate against. It
// mirrors how the host emits them: a flat node pool plus named defs.
type graphBuilder struct {
	nodes []types.SchemaTypeNode
	defs  []types.SchemaTypeDef
}

func (b *graphBuilder) push(body types.SchemaTypeBody) int32 {
	b.nodes = append(b.nodes, types.SchemaTypeNode{Body: body, Metadata: types.MetadataEnvelope{}})
	return int32(len(b.nodes) - 1)
}

// define names the node at body and returns a ref-type node pointing at it,
// which is how the host encodes a named (and possibly recursive) type.
func (b *graphBuilder) define(id string, body int32) int32 {
	b.defs = append(b.defs, types.SchemaTypeDef{
		Id:   id,
		Name: witTypes.None[string](),
		Body: body,
	})
	return b.push(types.MakeSchemaTypeBodyRefType(int32(len(b.defs) - 1)))
}

func (b *graphBuilder) graph(root int32) types.SchemaGraph {
	return types.SchemaGraph{TypeNodes: b.nodes, Defs: b.defs, Root: root}
}

type valueBuilder struct{ nodes []types.SchemaValueNode }

func (b *valueBuilder) push(n types.SchemaValueNode) int32 {
	b.nodes = append(b.nodes, n)
	return int32(len(b.nodes) - 1)
}

func (b *valueBuilder) tree(root int32) types.SchemaValueTree {
	return types.SchemaValueTree{ValueNodes: b.nodes, Root: root}
}

func some[T any](v T) witTypes.Option[T] { return witTypes.Some(v) }

// recordSchema builds `record { name: string, count: u32 }` and returns the
// graph plus the field indices, so tests can build matching values.
func recordSchema() types.SchemaGraph {
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	count := g.push(types.MakeSchemaTypeBodyU32Type(witTypes.None[types.NumericRestrictions]()))
	rec := g.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "name", Body: str, Metadata: types.MetadataEnvelope{}},
		{Name: "count", Body: count, Metadata: types.MetadataEnvelope{}},
	}))
	return g.graph(rec)
}

// TestValidateAcceptsAMatchingValue — the happy path over a record.
func TestValidateAcceptsAMatchingValue(t *testing.T) {
	ref := NewRef(recordSchema())

	var v valueBuilder
	name := v.push(types.MakeSchemaValueNodeStringValue("widget"))
	count := v.push(types.MakeSchemaValueNodeU32Value(7))
	root := v.push(types.MakeSchemaValueNodeRecordValue([]int32{name, count}))

	if err := ref.Validate(v.tree(root)); err != nil {
		t.Fatalf("valid value rejected: %v", err)
	}
}

// TestValidateReportsTheFieldPath — a mismatch deep in a value must name the
// field, not just the type, or a caller cannot tell which input was wrong.
func TestValidateReportsTheFieldPath(t *testing.T) {
	ref := NewRef(recordSchema())

	var v valueBuilder
	name := v.push(types.MakeSchemaValueNodeStringValue("widget"))
	count := v.push(types.MakeSchemaValueNodeStringValue("seven")) // wrong kind
	root := v.push(types.MakeSchemaValueNodeRecordValue([]int32{name, count}))

	err := ref.Validate(v.tree(root))
	if err == nil {
		t.Fatal("a string in a u32 field should not validate")
	}
	var ve *ValidationError
	if !asValidationError(err, &ve) {
		t.Fatalf("expected a *ValidationError, got %T", err)
	}
	if len(ve.Issues) != 1 {
		t.Fatalf("expected one issue, got %d: %v", len(ve.Issues), ve.Issues)
	}
	if ve.Issues[0].Path != "count" {
		t.Errorf("issue path = %q, want %q", ve.Issues[0].Path, "count")
	}
	if !strings.Contains(ve.Issues[0].Message, "u32") {
		t.Errorf("issue should name the expected type, got %q", ve.Issues[0].Message)
	}
}

// TestValidateCollectsEveryIssue — reporting only the first failure makes a
// caller fix its input one round-trip at a time.
func TestValidateCollectsEveryIssue(t *testing.T) {
	ref := NewRef(recordSchema())

	var v valueBuilder
	name := v.push(types.MakeSchemaValueNodeBoolValue(true))
	count := v.push(types.MakeSchemaValueNodeStringValue("seven"))
	root := v.push(types.MakeSchemaValueNodeRecordValue([]int32{name, count}))

	err := ref.Validate(v.tree(root))
	var ve *ValidationError
	if !asValidationError(err, &ve) {
		t.Fatalf("expected a *ValidationError, got %v", err)
	}
	if len(ve.Issues) != 2 {
		t.Fatalf("expected both fields reported, got %v", ve.Issues)
	}
}

// TestValidateRejectsAWrongFieldCount — a record value carries its fields
// positionally, so a missing one is not a missing name but a short list.
func TestValidateRejectsAWrongFieldCount(t *testing.T) {
	ref := NewRef(recordSchema())

	var v valueBuilder
	name := v.push(types.MakeSchemaValueNodeStringValue("widget"))
	root := v.push(types.MakeSchemaValueNodeRecordValue([]int32{name}))

	if err := ref.Validate(v.tree(root)); err == nil {
		t.Fatal("a record missing a field should not validate")
	}
}

// TestValidateRejectsAnOutOfRangeIndex — a malformed tree must be reported,
// not panic: these trees arrive from other components.
func TestValidateRejectsAnOutOfRangeIndex(t *testing.T) {
	ref := NewRef(recordSchema())

	var v valueBuilder
	name := v.push(types.MakeSchemaValueNodeStringValue("widget"))
	root := v.push(types.MakeSchemaValueNodeRecordValue([]int32{name, 99}))

	err := ref.Validate(v.tree(root))
	if err == nil {
		t.Fatal("an out-of-range value index should not validate")
	}
	if !strings.Contains(err.Error(), "out of range") {
		t.Errorf("error should explain the bad index, got %q", err)
	}
}

// TestValidateVariantPayloadAgreement — a case that declares a payload must
// carry one, and a case that declares none must not.
func TestValidateVariantPayloadAgreement(t *testing.T) {
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	variant := g.push(types.MakeSchemaTypeBodyVariantType([]types.VariantCaseType{
		{Name: "empty", Payload: witTypes.None[int32](), Metadata: types.MetadataEnvelope{}},
		{Name: "named", Payload: some(str), Metadata: types.MetadataEnvelope{}},
	}))
	ref := NewRef(g.graph(variant))

	t.Run("payload where none is declared", func(t *testing.T) {
		var v valueBuilder
		payload := v.push(types.MakeSchemaValueNodeStringValue("x"))
		root := v.push(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{
			Case: 0, Payload: some(payload),
		}))
		if err := ref.Validate(v.tree(root)); err == nil {
			t.Fatal("a payload on a payload-less case should not validate")
		}
	})

	t.Run("no payload where one is declared", func(t *testing.T) {
		var v valueBuilder
		root := v.push(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{
			Case: 1, Payload: witTypes.None[int32](),
		}))
		if err := ref.Validate(v.tree(root)); err == nil {
			t.Fatal("a missing payload should not validate")
		}
	})

	t.Run("case out of range", func(t *testing.T) {
		var v valueBuilder
		root := v.push(types.MakeSchemaValueNodeVariantValue(types.VariantValuePayload{
			Case: 9, Payload: witTypes.None[int32](),
		}))
		if err := ref.Validate(v.tree(root)); err == nil {
			t.Fatal("an unknown case should not validate")
		}
	})
}

// TestValidateFollowsNamedDefinitions — a ref-type node must resolve to the
// definition it names before the value is checked.
func TestValidateFollowsNamedDefinitions(t *testing.T) {
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	named := g.define("demo.Name", str)
	ref := NewRef(g.graph(named))

	var v valueBuilder
	root := v.push(types.MakeSchemaValueNodeStringValue("through the ref"))
	if err := ref.Validate(v.tree(root)); err != nil {
		t.Fatalf("value behind a named def rejected: %v", err)
	}
}

// TestRecursiveSchemaTerminates — a type that contains itself (a tree node
// whose children are trees) must not send traversal into a loop.
func TestRecursiveSchemaTerminates(t *testing.T) {
	// node = record { value: string, children: list<node> }
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	placeholder := g.push(types.MakeSchemaTypeBodyRefType(0)) // def 0, filled below
	list := g.push(types.MakeSchemaTypeBodyListType(placeholder))
	rec := g.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "value", Body: str, Metadata: types.MetadataEnvelope{}},
		{Name: "children", Body: list, Metadata: types.MetadataEnvelope{}},
	}))
	g.defs = append(g.defs, types.SchemaTypeDef{
		Id: "demo.Node", Name: witTypes.None[string](), Body: rec,
	})
	ref := NewRef(g.graph(placeholder))

	if got := ref.TypeName(); got != "demo.Node" {
		t.Errorf("a named type should render as its name, got %q", got)
	}
	if ref.ContainsStream() {
		t.Error("a recursive record with no stream reports one")
	}

	// A two-level tree validates without looping.
	var v valueBuilder
	leafValue := v.push(types.MakeSchemaValueNodeStringValue("leaf"))
	noChildren := v.push(types.MakeSchemaValueNodeListValue(nil))
	leaf := v.push(types.MakeSchemaValueNodeRecordValue([]int32{leafValue, noChildren}))
	rootValue := v.push(types.MakeSchemaValueNodeStringValue("root"))
	children := v.push(types.MakeSchemaValueNodeListValue([]int32{leaf}))
	root := v.push(types.MakeSchemaValueNodeRecordValue([]int32{rootValue, children}))

	if err := ref.Validate(v.tree(root)); err != nil {
		t.Fatalf("recursive value rejected: %v", err)
	}
}

// TestContainsStreamFindsNestedStreams — trigger and schedule are refused for
// stream-bearing methods, so the check has to see through composites.
func TestContainsStreamFindsNestedStreams(t *testing.T) {
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	stream := g.push(types.MakeSchemaTypeBodyStreamType(some(str)))
	inner := g.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "chunks", Body: stream, Metadata: types.MetadataEnvelope{}},
	}))
	list := g.push(types.MakeSchemaTypeBodyListType(inner))
	outer := g.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "batches", Body: list, Metadata: types.MetadataEnvelope{}},
	}))

	if !NewRefAt(g.graph(outer), outer).ContainsStream() {
		t.Error("a stream nested in a list of records was not found")
	}
	if NewRefAt(g.graph(outer), str).ContainsStream() {
		t.Error("a plain string reports a stream")
	}
}

// TestTypeNameRendersComposites — the rendering is what a user sees when a
// dynamic call is rejected, so it must be readable.
func TestTypeNameRendersComposites(t *testing.T) {
	var g graphBuilder
	str := g.push(types.MakeSchemaTypeBodyStringType())
	u32 := g.push(types.MakeSchemaTypeBodyU32Type(witTypes.None[types.NumericRestrictions]()))
	opt := g.push(types.MakeSchemaTypeBodyOptionType(str))
	res := g.push(types.MakeSchemaTypeBodyResultType(types.ResultSpec{
		Ok: some(u32), Err: some(str),
	}))
	rec := g.push(types.MakeSchemaTypeBodyRecordType([]types.NamedFieldType{
		{Name: "label", Body: opt, Metadata: types.MetadataEnvelope{}},
		{Name: "outcome", Body: res, Metadata: types.MetadataEnvelope{}},
	}))

	got := NewRef(g.graph(rec)).TypeName()
	want := "record{label: option<string>, outcome: result<u32, string>}"
	if got != want {
		t.Errorf("TypeName() = %q, want %q", got, want)
	}
}

// asValidationError is errors.As without importing errors into every test.
func asValidationError(err error, target **ValidationError) bool {
	ve, ok := err.(*ValidationError)
	if ok {
		*target = ve
	}
	return ok
}
