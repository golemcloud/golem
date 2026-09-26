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
	"fmt"
	"strings"
)

// Ref points at one type within a graph, so a walker can carry the graph along
// with the node it is looking at. Resolving a reference stays inside the same
// graph, which is what makes a graph self-contained.
type Ref struct {
	graph SchemaGraph
	typ   SchemaType
}

// NewRef points at a graph's root.
func NewRef(g SchemaGraph) Ref { return Ref{graph: g, typ: g.Root} }

// NewRefAt points at a type read against a graph.
func NewRefAt(g SchemaGraph, t SchemaType) Ref { return Ref{graph: g, typ: t} }

// Graph returns the graph this reference resolves against.
func (r Ref) Graph() SchemaGraph { return r.graph }

// Type returns the type without following references.
func (r Ref) Type() SchemaType { return r.typ }

// At points at another type in the same graph.
func (r Ref) At(t SchemaType) Ref { return Ref{graph: r.graph, typ: t} }

// maxRefChain bounds reference following. A graph whose references form a cycle
// with no type in between is malformed; without a bound it would hang here
// rather than being reported.
const maxRefChain = 512

// body follows any chain of references and returns the structural body.
func (r Ref) body() (SchemaTypeBody, error) {
	t := r.typ
	for i := 0; i < maxRefChain; i++ {
		ref, isRef := t.Body.(RefType)
		if !isRef {
			return t.Body, nil
		}
		def, found := r.graph.Def(ref.Id)
		if !found {
			return nil, fmt.Errorf("golem: unknown type %q", ref.Id)
		}
		t = def.Body
	}
	return nil, fmt.Errorf("golem: type reference does not resolve after %d steps", maxRefChain)
}

// Resolved follows any chain of references and points at what it finds.
func (r Ref) Resolved() (Ref, error) {
	b, err := r.body()
	if err != nil {
		return Ref{}, err
	}
	return Ref{graph: r.graph, typ: SchemaType{Body: b, Metadata: r.typ.Metadata}}, nil
}

// ContainsStream reports whether a stream appears anywhere inside the type.
// Cardinality-changing invocation forms need to know: a call that returns
// before it completes cannot hand back a stream endpoint.
func (r Ref) ContainsStream() bool {
	return r.containsStream(map[string]bool{})
}

func (r Ref) containsStream(seen map[string]bool) bool {
	if ref, isRef := r.typ.Body.(RefType); isRef {
		if seen[ref.Id] {
			// Already on this path: a cycle cannot introduce a stream that the
			// first visit did not already see.
			return false
		}
		seen[ref.Id] = true
	}
	b, err := r.body()
	if err != nil {
		return false
	}
	if _, isStream := b.(StreamType); isStream {
		return true
	}
	for _, child := range r.children(b) {
		if r.At(child).containsStream(seen) {
			return true
		}
	}
	return false
}

// children returns the types directly reachable from a body.
func (r Ref) children(b SchemaTypeBody) []SchemaType {
	switch t := b.(type) {
	case RecordType:
		out := make([]SchemaType, 0, len(t.Fields))
		for _, f := range t.Fields {
			out = append(out, f.Body)
		}
		return out
	case VariantType:
		var out []SchemaType
		for _, c := range t.Cases {
			if c.Payload != nil {
				out = append(out, *c.Payload)
			}
		}
		return out
	case TupleType:
		return t.Elements
	case ListType:
		return []SchemaType{t.Element}
	case FixedListType:
		return []SchemaType{t.Element}
	case MapType:
		return []SchemaType{t.Key, t.Value}
	case OptionType:
		return []SchemaType{t.Inner}
	case ResultType:
		var out []SchemaType
		if t.Ok != nil {
			out = append(out, *t.Ok)
		}
		if t.Err != nil {
			out = append(out, *t.Err)
		}
		return out
	case UnionType:
		out := make([]SchemaType, 0, len(t.Branches))
		for _, br := range t.Branches {
			out = append(out, br.Body)
		}
		return out
	case SecretType:
		return []SchemaType{t.Inner}
	case FutureType:
		if t.Item != nil {
			return []SchemaType{*t.Item}
		}
	case StreamType:
		if t.Item != nil {
			return []SchemaType{*t.Item}
		}
	}
	return nil
}

// TypeName renders the type for a human. A named definition renders as its id,
// which is also what stops a recursive type rendering forever.
func (r Ref) TypeName() string {
	switch t := r.typ.Body.(type) {
	case RefType:
		return t.Id
	case BoolType:
		return "bool"
	case CharType:
		return "char"
	case StringType:
		return "string"
	case S8Type:
		return "s8"
	case S16Type:
		return "s16"
	case S32Type:
		return "s32"
	case S64Type:
		return "s64"
	case U8Type:
		return "u8"
	case U16Type:
		return "u16"
	case U32Type:
		return "u32"
	case U64Type:
		return "u64"
	case F32Type:
		return "f32"
	case F64Type:
		return "f64"
	case TextType:
		return "text"
	case BinaryType:
		return "binary"
	case PathType:
		return "path"
	case UrlType:
		return "url"
	case DatetimeType:
		return "datetime"
	case DurationType:
		return "duration"
	case QuantityType:
		return "quantity<" + t.Spec.BaseUnit + ">"
	case EnumType:
		return "enum{" + strings.Join(t.Cases, ", ") + "}"
	case FlagsType:
		return "flags{" + strings.Join(t.Flags, ", ") + "}"
	case RecordType:
		parts := make([]string, 0, len(t.Fields))
		for _, f := range t.Fields {
			parts = append(parts, f.Name+": "+r.At(f.Body).TypeName())
		}
		return "record{" + strings.Join(parts, ", ") + "}"
	case VariantType:
		parts := make([]string, 0, len(t.Cases))
		for _, c := range t.Cases {
			if c.Payload == nil {
				parts = append(parts, c.Name)
				continue
			}
			parts = append(parts, c.Name+"("+r.At(*c.Payload).TypeName()+")")
		}
		return "variant{" + strings.Join(parts, ", ") + "}"
	case UnionType:
		parts := make([]string, 0, len(t.Branches))
		for _, b := range t.Branches {
			parts = append(parts, b.Tag+"("+r.At(b.Body).TypeName()+")")
		}
		return "union{" + strings.Join(parts, ", ") + "}"
	case TupleType:
		parts := make([]string, 0, len(t.Elements))
		for _, e := range t.Elements {
			parts = append(parts, r.At(e).TypeName())
		}
		return "tuple<" + strings.Join(parts, ", ") + ">"
	case ListType:
		return "list<" + r.At(t.Element).TypeName() + ">"
	case FixedListType:
		return fmt.Sprintf("list<%s; %d>", r.At(t.Element).TypeName(), t.Length)
	case MapType:
		return "map<" + r.At(t.Key).TypeName() + ", " + r.At(t.Value).TypeName() + ">"
	case OptionType:
		return "option<" + r.At(t.Inner).TypeName() + ">"
	case ResultType:
		return "result<" + r.sideName(t.Ok) + ", " + r.sideName(t.Err) + ">"
	case SecretType:
		return "secret<" + r.At(t.Inner).TypeName() + ">"
	case QuotaTokenType:
		return "quota-token"
	case PermissionCardType:
		return "permission-card"
	case FutureType:
		return "future<" + r.itemName(t.Item) + ">"
	case StreamType:
		return "stream<" + r.itemName(t.Item) + ">"
	}
	return "unknown"
}

func (r Ref) sideName(t *SchemaType) string {
	if t == nil {
		return "_"
	}
	return r.At(*t).TypeName()
}

func (r Ref) itemName(t *SchemaType) string {
	if t == nil {
		return "?"
	}
	return r.At(*t).TypeName()
}
