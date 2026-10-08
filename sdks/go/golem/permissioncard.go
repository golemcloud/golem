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
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"reflect"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Permission cards.
//
// A permission card is delegated authority, received from the host or another
// call and handed on as a method or tool argument or result. It is opaque —
// there is nothing to read or construct — and affine: sending a card moves it,
// and the copy that was sent is unusable afterwards, even when the call fails.
//
//	type ForwardIn struct{ Card golem.PermissionCard }
//
//	var _ = agent.Handle(Forward, func(_ *golem.Context[state], in ForwardIn) golem.PermissionCard {
//	    return Accept.MustCall(Receiver.Get(ReceiverID{Name: "target"}), AcceptIn{Card: in.Card})
//	})

// ErrPermissionCardMoved reports a card used after it was handed on.
var ErrPermissionCardMoved = errors.New("golem: the permission card was moved")

// PermissionCard is a card whose grants name concrete owners and resources.
type PermissionCard struct{ st *permissionCardState }

// PolymorphicPermissionCard is a card whose grants may leave the owner or the
// resource id open, to be filled in where it is used.
type PolymorphicPermissionCard struct{ st *permissionCardState }

type permissionCardState struct{ h *types.PermissionCard }

// cardish recognises the card types in the deriver.
type cardish interface {
	cardState() *permissionCardState
	polymorphic() bool
}

func (c PermissionCard) cardState() *permissionCardState            { return c.st }
func (c PermissionCard) polymorphic() bool                          { return false }
func (c PolymorphicPermissionCard) cardState() *permissionCardState { return c.st }
func (c PolymorphicPermissionCard) polymorphic() bool               { return true }

// compilePermissionCard lowers a card type to the WIT permission-card type.
// Encoding moves the card; decoding adopts the received one.
func compilePermissionCard(c *engine.Codec, polymorphic bool) {
	c.Body = func(*engine.GraphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyPermissionCardType(types.PermissionCardSpec{Polymorphic: polymorphic})
	}
	c.Encode = func(b *engine.ValBuilder, v reflect.Value) int32 {
		st := v.Interface().(cardish).cardState()
		if st == nil || st.h == nil {
			panic(&engine.EncodeError{Msg: ErrPermissionCardMoved.Error()})
		}
		h := st.h
		st.h = nil
		return b.Push(types.MakeSchemaValueNodePermissionCardHandle(h))
	}
	c.Decode = func(d *engine.Decoder, dst reflect.Value, idx int32) error {
		n, err := d.Node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodePermissionCardHandle {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.Typ)
		}
		st := &permissionCardState{h: n.PermissionCardHandle()}
		if polymorphic {
			dst.Set(reflect.ValueOf(PolymorphicPermissionCard{st: st}))
		} else {
			dst.Set(reflect.ValueOf(PermissionCard{st: st}))
		}
		return nil
	}
}
