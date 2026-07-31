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
	"fmt"
	"reflect"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

func scalar(
	c *codec,
	body types.SchemaTypeBody,
	tag uint8,
	enc func(*valBuilder, reflect.Value) int32,
	set func(reflect.Value, types.SchemaValueNode),
) {
	c.body = func(*graphBuilder) types.SchemaTypeBody { return body }
	c.encode = enc
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != tag {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		set(dst, n)
		return nil
	}
}

// compileRecord handles Go structs, which lower to WIT records. Fields are
// positional on the wire: declaration order is the order the schema reports and
// the order values are written in.

type optionOps struct {
	// get reports whether the value is present, and if so yields the inner value.
	get func(v reflect.Value) (reflect.Value, bool)
	// setNone clears dst.
	setNone func(dst reflect.Value)
	// setSome prepares dst to hold a value and returns the slot to decode into.
	setSome func(dst reflect.Value) reflect.Value
}

func pointerOps(t reflect.Type) optionOps {
	return optionOps{
		get: func(v reflect.Value) (reflect.Value, bool) {
			if v.IsNil() {
				return reflect.Value{}, false
			}
			return v.Elem(), true
		},
		setNone: func(dst reflect.Value) { dst.Set(reflect.Zero(t)) },
		setSome: func(dst reflect.Value) reflect.Value {
			p := reflect.New(t.Elem())
			dst.Set(p)
			return p.Elem()
		},
	}
}

func compileOption(c *codec, inner *codec, ops optionOps) {
	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyOptionType(g.node(inner))
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		if in, some := ops.get(v); some {
			idx := inner.encode(b, in)
			return b.push(types.MakeSchemaValueNodeOptionValue(witTypes.Some(idx)))
		}
		return b.push(types.MakeSchemaValueNodeOptionValue(witTypes.None[int32]()))
	}
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeOptionValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		opt := n.OptionValue()
		if opt.IsNone() {
			ops.setNone(dst)
			return nil
		}
		return inner.decode(d, ops.setSome(dst), opt.Some())
	}
}

func optionValueOps() optionOps {
	return optionOps{
		get: func(v reflect.Value) (reflect.Value, bool) {
			return v.Interface().(optionish).optionGet()
		},
		setNone: func(dst reflect.Value) {
			dst.Addr().Interface().(optionSetter).optionSetNone()
		},
		setSome: func(dst reflect.Value) reflect.Value {
			return dst.Addr().Interface().(optionSetter).optionSetSome()
		},
	}
}

func compileSecret(c *codec, inner *codec) {
	// Schema side only: emit the secret(inner) type node. This is what the config
	// graph and the config-metadata declaration need. inner is compiled so the
	// node references the revealed type.
	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodySecretType(types.SecretSpec{
			Inner:    g.node(inner),
			Category: witTypes.None[string](),
		})
	}
	// Secrets are config-only: they are read via get-config-value + reveal (see
	// readSecretValue), never carried as plaintext in an invocation payload. Guard
	// the wire path so using a Secret[T] as a method parameter/return fails clearly
	// instead of silently shipping plaintext. Config never reaches these — it uses
	// c.body for the graph and reveal for the value.
	c.encode = func(*valBuilder, reflect.Value) int32 {
		panic(&encodeError{"Secret[T] is config-only; it cannot be a method parameter or return value"})
	}
	c.decode = func(*decoder, reflect.Value, int32) error {
		return fmt.Errorf("golem: Secret[T] is config-only; it cannot be a method parameter or return value")
	}
}
