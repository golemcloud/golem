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
	"errors"
	"fmt"
	"reflect"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	"github.com/golemcloud/golem/sdks/go/golem/schema"
)

// TypedValue is a value travelling with its own schema. Code that handles other
// components' values — middleware, in particular — cannot know their Go types,
// so it reads and rebuilds them through the schema instead.
type TypedValue struct{ wit types.TypedSchemaValue }

// Schema returns a reference to the value's type, for validation or rendering.
func (v TypedValue) Schema() schema.Ref { return schema.NewRef(v.wit.Graph) }

// JSON decodes the value into ordinary Go values, in the host's canonical form.
func (v TypedValue) JSON() (any, error) { return v.Schema().UnpackJSON(v.wit.Value) }

// WithJSON rebuilds the value from canonical JSON against the same schema, which
// is how a middleware rewrites an argument it does not have a Go type for.
func (v TypedValue) WithJSON(value any) (TypedValue, error) {
	tree, err := v.Schema().PackJSON(value)
	if err != nil {
		return TypedValue{}, err
	}
	return TypedValue{wit: types.TypedSchemaValue{Graph: v.wit.Graph, Value: tree}}, nil
}

// Decode reads the value into a Go value of type T, whose schema must match.
func DecodeTypedValue[T any](v TypedValue) (T, error) {
	var out T
	c := defs.compile(reflect.TypeFor[T]())
	if c.invalid != "" {
		return out, fmt.Errorf("golem: %s cannot be decoded: %s", reflect.TypeFor[T](), c.invalid)
	}
	d := decoder{nodes: v.wit.Value.ValueNodes}
	slot := reflect.ValueOf(&out).Elem()
	if err := c.decode(&d, slot, v.wit.Value.Root); err != nil {
		return out, err
	}
	return out, nil
}

// EncodeTypedValue packages a Go value together with its derived schema.
func EncodeTypedValue[T any](value T) (TypedValue, error) {
	c := defs.compile(reflect.TypeFor[T]())
	if c.invalid != "" {
		return TypedValue{}, fmt.Errorf("golem: %s cannot be encoded: %s", reflect.TypeFor[T](), c.invalid)
	}
	g := graphBuilder{d: defs}
	root := g.node(c)
	graph := g.build()
	graph.Root = root
	return TypedValue{wit: types.TypedSchemaValue{
		Graph: graph,
		Value: encodeWith(c, reflect.ValueOf(&value).Elem()),
	}}, nil
}

// errorsAs is errors.As, wrapped so the middleware dispatcher does not have to
// import errors just for one call.
func errorsAs(err error, target any) bool { return errors.As(err, target) }
