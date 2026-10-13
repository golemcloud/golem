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

	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// sdkComposites recognise the SDK's own struct types that are not plain
// records: secrets, agent streams, quota tokens and permission cards.
var sdkComposites = []func(e *engine.Engine, c *engine.Codec, zero any) bool{
	func(e *engine.Engine, c *engine.Codec, zero any) bool {
		switch z := zero.(type) {
		case secretish:
			compileSecret(c, e.Compile(z.secretElem()))
		case streamish:
			compileStream(c, e.Compile(z.streamElem()))
		case quotaish:
			compileQuotaToken(c)
		case cardish:
			compilePermissionCard(c, z.polymorphic())
		default:
			return false
		}
		return true
	},
}

func compileSecret(c *engine.Codec, inner *engine.Codec) {
	// Schema side only: emit the secret(inner) type node. This is what the config
	// graph and the config-metadata declaration need. inner is compiled so the
	// node references the revealed type.
	c.Body = func(g *engine.GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodySecretType(types.SecretSpec{
			Inner:    g.Node(inner),
			Category: witTypes.None[string](),
		})
	}
	// An invocation carries a secret as a handle, never as plaintext: sending
	// one hands a handle on, and a received one is revealed through the host.
	c.Encode = func(b *engine.ValBuilder, v reflect.Value) int32 {
		h, err := v.Interface().(secretTaker).secretTake()
		if err != nil {
			panic(&engine.EncodeError{Msg: err.Error()})
		}
		return b.Push(types.MakeSchemaValueNodeSecretValue(h))
	}
	c.Decode = func(d *engine.Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeSecretValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		fresh := reflect.New(c.Typ)
		fresh.Interface().(secretAdopter).secretAdopt(n.SecretValue())
		dst.Set(fresh.Elem())
		return nil
	}
}
