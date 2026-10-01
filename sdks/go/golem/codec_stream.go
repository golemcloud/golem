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
)

// streamish lets the deriver recognise an AgentStream without knowing its item
// type, resolved once against the zero value like the other SDK composites.
type streamish interface {
	streamElem() reflect.Type
	streamTake() (treeSource, error)
	streamAdopt(src treeSource)
}

func (s AgentStream[T]) streamElem() reflect.Type { return reflect.TypeFor[T]() }

// streamTake hands the reading endpoint on. It is the affine move: the stream
// is unusable afterwards, through this copy or any other, because they share
// the state.
func (s AgentStream[T]) streamTake() (treeSource, error) {
	if s.st == nil {
		return treeSource{}, ErrStreamClosed
	}
	switch {
	case s.st.taken:
		return treeSource{}, ErrStreamTransferred
	case s.st.closed:
		return treeSource{}, ErrStreamClosed
	case s.st.started:
		// The items already read cannot be put back, so the receiver would get
		// a different stream from the one it was promised.
		return treeSource{}, ErrStreamPartiallyRead
	}
	s.st.taken = true
	return s.st.src, nil
}

func (s AgentStream[T]) streamAdopt(src treeSource) { s.st.src = src }

// compileStream lowers AgentStream[T] to the WIT stream type.
func compileStream(c *codec, inner *codec) {
	c.containsStream = true
	c.body = func(g *graphBuilder) types.SchemaTypeBody {
		// Always typed: Go has no way to spell an untyped stream, and an
		// untyped one could not supply an item codec anyway.
		return streamTypeBody(g.node(inner))
	}

	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		src, err := v.Interface().(streamish).streamTake()
		if err != nil {
			panic(&encodeError{err.Error()})
		}
		node, ok := streamNodeFrom(src)
		if !ok {
			panic(&encodeError{"a stream can only be transferred from inside a component"})
		}
		return b.push(node)
	}

	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeStreamValue {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		// The endpoint is taken now but not read: forwarding a stream that was
		// never read must stay a pure move, with no items pumped through here.
		src, ok := streamSourceFromNode(n)
		if !ok {
			return fmt.Errorf("golem: a stream can only be received inside a component")
		}
		fresh := reflect.New(c.typ)
		fresh.Interface().(streamReceiver).streamReceive(src)
		dst.Set(fresh.Elem())
		return nil
	}
}

// streamReceiver initialises a zero AgentStream with a received endpoint. Its
// state is unexported, and reflection cannot set an unexported field, so the
// decoder goes through this method on a pointer to a fresh value instead.
type streamReceiver interface {
	streamReceive(src treeSource)
}

func (s *AgentStream[T]) streamReceive(src treeSource) {
	s.st = &streamState{src: src}
	s.codec = defaultStreamCodec[T]()
}
