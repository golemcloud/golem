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

//go:build wasip1

package witschema

import (
	"fmt"

	core "github.com/golemcloud/golem/sdks/go/core/schema"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// The handle adapters are the only place a generated resource is bound to an
// interface. Doing that materializes the resource's Drop method, which is a
// host call, so a shared file would drag //go:wasmimport symbols into a native
// link that has no bodies for them.

type secretHandle struct{ h *types.Secret }

func (s secretHandle) Release() { s.h.Drop() }

type quotaHandle struct{ h *types.QuotaToken }

func (q quotaHandle) Release() { q.h.Drop() }

type cardHandle struct{ h *types.PermissionCard }

func (c cardHandle) Release() { c.h.Drop() }

type streamHandle struct{ h *types.SchemaValueStream }

func (s streamHandle) Release() { s.h.Drop() }

func handleToCore(n types.SchemaValueNode) (core.SchemaValue, error) {
	switch n.Tag() {
	case types.SchemaValueNodeSecretValue:
		return core.SecretValue{Handle: secretHandle{h: n.SecretValue()}}, nil
	case types.SchemaValueNodeQuotaTokenHandle:
		return core.QuotaTokenValue{Handle: quotaHandle{h: n.QuotaTokenHandle()}}, nil
	case types.SchemaValueNodePermissionCardHandle:
		return core.PermissionCardValue{Handle: cardHandle{h: n.PermissionCardHandle()}}, nil
	case types.SchemaValueNodeStreamValue:
		return core.StreamValue{Handle: streamHandle{h: n.StreamValue()}}, nil
	}
	return nil, fmt.Errorf("golem: value node (tag %d) is not a host-managed handle", n.Tag())
}

func handleToWit(v core.SchemaValue) (types.SchemaValueNode, error) {
	switch t := v.(type) {
	case core.SecretValue:
		h, ok := t.Handle.(secretHandle)
		if !ok {
			return types.SchemaValueNode{}, errForeignHandle("secret")
		}
		return types.MakeSchemaValueNodeSecretValue(h.h), nil
	case core.QuotaTokenValue:
		h, ok := t.Handle.(quotaHandle)
		if !ok {
			return types.SchemaValueNode{}, errForeignHandle("quota token")
		}
		return types.MakeSchemaValueNodeQuotaTokenHandle(h.h), nil
	case core.PermissionCardValue:
		h, ok := t.Handle.(cardHandle)
		if !ok {
			return types.SchemaValueNode{}, errForeignHandle("permission card")
		}
		return types.MakeSchemaValueNodePermissionCardHandle(h.h), nil
	case core.StreamValue:
		h, ok := t.Handle.(streamHandle)
		if !ok {
			return types.SchemaValueNode{}, errForeignHandle("stream")
		}
		return types.MakeSchemaValueNodeStreamValue(h.h), nil
	}
	return types.SchemaValueNode{}, fmt.Errorf("golem: %T is not a host-managed handle", v)
}

// errForeignHandle reports a handle that did not come from this host. Only one
// the platform lent out can be handed back to it.
func errForeignHandle(kind string) error {
	return fmt.Errorf("golem: this %s handle did not come from the host and cannot be sent to it", kind)
}
