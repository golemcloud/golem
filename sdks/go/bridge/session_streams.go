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

package bridge

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"

	"github.com/golemcloud/golem/sdks/go/core/schema"
)

// --- inputs ----------------------------------------------------------------

// inputStream is a stream argument being sent. One range of items is in flight
// at a time; the next is read from the source only after the server durably
// accepted the last, so whatever a reconnect loses is exactly that range.
type inputStream struct {
	s           *session
	h           *inputHandle
	provisional string
	token       string
	channel     uint32

	next         uint64
	pending      *pendingInput
	ended        bool
	cancelled    bool
	cancelReason string
	pumping      bool
	// endAfter is set when the source ended while a packed batch was being
	// filled; the end is sent after that batch.
	endAfter bool
	wake     chan struct{}
}

type pendingInput struct {
	first    uint64
	count    uint64
	terminal bool
	lane     lane
	value    json.RawMessage
	payload  []byte
	mime     *string
	bytes    int
}

func (p *pendingInput) frame(channel uint32) (outFrame, error) {
	sequence := strconv.FormatUint(p.first, 10)
	switch {
	case p.terminal:
		data, err := encodeMessage("inputStreamEnd", map[string]any{"channel": channel, "sequence": sequence})
		return outFrame{data: data}, err
	case p.lane == laneU8:
		data, err := encodeBinary("input-u8", channel, p.first, len(p.payload), nil, p.payload)
		return outFrame{binary: true, data: data}, err
	case p.lane == laneBinary:
		data, err := encodeBinary("input-binary", channel, p.first, 1, p.mime, p.payload)
		return outFrame{binary: true, data: data}, err
	}
	data, err := encodeMessage("inputStreamItem", map[string]any{
		"channel": channel, "sequence": sequence, "value": p.value,
	})
	return outFrame{data: data}, err
}

// sendPending sends the range in flight, if the stream has a channel. Called
// with s.mu held.
func (in *inputStream) sendPending() {
	if in.pending == nil || in.channel == 0 || in.s.conn == nil {
		return
	}
	f, err := in.pending.frame(in.channel)
	if err != nil {
		go in.s.fail(err)
		return
	}
	in.s.conn.enqueue(f)
}

// release forgets the range in flight. Called with s.mu held.
func (in *inputStream) release() {
	if in.pending == nil {
		return
	}
	in.s.unacked -= in.pending.bytes
	in.pending = nil
	for _, other := range in.s.byToken {
		poke(other.wake)
	}
	poke(in.wake)
}

// trim drops what the server accepted from a partly accepted range. Only a
// packed byte batch can be split. Called with s.mu held.
func (in *inputStream) trim(high uint64) error {
	p := in.pending
	if p.lane != laneU8 || p.terminal {
		return protocolErrorf("the server split an item that is sent whole")
	}
	accepted := high - p.first
	in.s.unacked -= int(accepted)
	p.payload = p.payload[accepted:]
	p.first = high
	p.count -= accepted
	p.bytes -= int(accepted)
	return nil
}

// remap binds the stream to a channel of a new attachment and resends what
// the server has not durably accepted. Called with s.mu held.
func (in *inputStream) remap(channel uint32, high uint64, terminal bool) error {
	if high > in.next {
		return protocolErrorf("the input high-water is beyond what was sent")
	}
	p := in.pending
	if p != nil && !p.terminal && (high < p.first || high > p.first+p.count) {
		return protocolErrorf("the input high-water conflicts with the range in flight")
	}
	if terminal && !in.ended {
		return protocolErrorf("the server reports an input end that was never sent")
	}
	if !terminal && in.ended && !in.cancelled && p == nil {
		return protocolErrorf("the server lost a durably accepted input end")
	}
	in.channel = channel
	if p != nil {
		switch {
		case p.terminal && terminal, !p.terminal && high == p.first+p.count:
			in.release()
		case !p.terminal && high > p.first:
			if err := in.trim(high); err != nil {
				return err
			}
		}
	}
	in.sendPending()
	if in.cancelReason != "" && !terminal {
		in.s.sendCancel(channel, in.cancelReason)
	}
	if !in.pumping && !in.ended {
		in.pumping = true
		go in.pump()
	}
	poke(in.wake)
	return nil
}

// acknowledge applies a cumulative acknowledgement. Called with s.mu held.
func (in *inputStream) acknowledge(high uint64, terminal bool) error {
	if high > in.next {
		return protocolErrorf("an input acknowledgement is beyond what was sent")
	}
	p := in.pending
	if p == nil {
		if high != in.next {
			return protocolErrorf("a duplicate input acknowledgement conflicts with what was accepted")
		}
		return nil
	}
	if !p.terminal && (high < p.first || high > p.first+p.count) {
		return protocolErrorf("an input acknowledgement conflicts with the range in flight")
	}
	if terminal && !in.ended {
		return protocolErrorf("an input acknowledgement reports an end that was never sent")
	}
	switch {
	case p.terminal && terminal && high == in.next, !p.terminal && high == p.first+p.count:
		in.release()
	case !p.terminal && high > p.first:
		return in.trim(high)
	}
	return nil
}

func (in *inputStream) pump() {
	for {
		if !in.idle() {
			return
		}
		p, err := in.produce()
		if err != nil {
			if in.s.life.Err() == nil {
				in.unavailable()
			}
			return
		}
		if !in.reserve(p) {
			return
		}
	}
}

// idle waits until the next range may be read: nothing in flight, and the
// session's unacknowledged budget not spent.
func (in *inputStream) idle() bool {
	s := in.s
	s.mu.Lock()
	for {
		if s.over || in.ended {
			s.mu.Unlock()
			return false
		}
		if in.pending == nil && s.unacked < maxUnacked {
			s.mu.Unlock()
			return true
		}
		s.mu.Unlock()
		select {
		case <-in.wake:
		case <-s.done:
		}
		s.mu.Lock()
	}
}

func (in *inputStream) produce() (*pendingInput, error) {
	if in.endAfter {
		return &pendingInput{terminal: true}, nil
	}
	v, ok, err := in.h.next(in.s.life)
	if err != nil {
		return nil, err
	}
	if !ok {
		return &pendingInput{terminal: true}, nil
	}
	switch in.h.lane {
	case laneU8:
		b, ok := v.(schema.U8Value)
		if !ok {
			return nil, Mismatch("u8", v)
		}
		payload := []byte{b.Value}
		for len(payload) < maxPackedBytes && in.h.ready() {
			v, ok, err := in.h.next(in.s.life)
			if err != nil {
				return nil, err
			}
			if !ok {
				in.endAfter = true
				break
			}
			b, isByte := v.(schema.U8Value)
			if !isByte {
				return nil, Mismatch("u8", v)
			}
			payload = append(payload, b.Value)
		}
		return &pendingInput{count: uint64(len(payload)), lane: laneU8, payload: payload, bytes: len(payload)}, nil
	case laneBinary:
		b, ok := v.(schema.BinaryValue)
		if !ok {
			return nil, Mismatch("binary", v)
		}
		if len(b.Bytes) > maxBinaryBytes {
			return nil, errors.New("golem: a binary stream item exceeds 16 MiB")
		}
		if b.MimeType != nil && !mimePattern.MatchString(*b.MimeType) {
			return nil, fmt.Errorf("golem: %q is not a MIME type", *b.MimeType)
		}
		return &pendingInput{count: 1, lane: laneBinary, payload: b.Bytes, mime: b.MimeType, bytes: len(b.Bytes)}, nil
	}
	in.s.mu.Lock()
	v, err = in.s.registerNested(v)
	in.s.mu.Unlock()
	if err != nil {
		return nil, err
	}
	raw, err := schema.MarshalWireValue(v)
	if err != nil {
		return nil, err
	}
	if len(raw) > maxBinaryBytes {
		return nil, errors.New("golem: a stream item exceeds 16 MiB")
	}
	return &pendingInput{count: 1, lane: laneJSON, value: raw, bytes: len(raw)}, nil
}

func (in *inputStream) reserve(p *pendingInput) bool {
	s := in.s
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.over || in.cancelled {
		return false
	}
	p.first = in.next
	in.next += p.count
	in.pending = p
	s.unacked += p.bytes
	if p.terminal {
		in.ended = true
	}
	in.sendPending()
	return true
}

// unavailable reports a source that failed: the server sees the input
// cancelled as source-unavailable.
func (in *inputStream) unavailable() {
	s := in.s
	s.mu.Lock()
	defer s.mu.Unlock()
	in.cancelReason = "source-unavailable"
	in.cancelled = true
	in.ended = true
	in.release()
	if in.channel != 0 {
		s.sendCancel(in.channel, in.cancelReason)
	}
}

// registerNested gives each stream inside an input item a provisional
// reference; the server maps it in the item's acknowledgement. Called with
// s.mu held.
func (s *session) registerNested(v schema.SchemaValue) (schema.SchemaValue, error) {
	return mapStreams(v, func(sv schema.StreamValue) (schema.SchemaValue, error) {
		h, ok := sv.Handle.(*inputHandle)
		if !ok {
			return nil, errors.New("golem: a stream inside a stream item must be a bridge.AgentStream")
		}
		if h.err != nil {
			return nil, h.err
		}
		in := &inputStream{s: s, h: h, provisional: newUUID(), wake: make(chan struct{}, 1)}
		s.byProvisional[in.provisional] = in
		return schema.StreamValue{Handle: schema.WireStreamRef{ProvisionalRef: in.provisional}}, nil
	})
}

// --- outputs ---------------------------------------------------------------

// outputStream is a stream the server returned. Items wait in a bounded
// queue; a full queue stops the session reading, which is how a slow reader
// slows the agent down.
type outputStream struct {
	s       *session
	token   string
	channel uint32
	item    schema.Ref
	lane    lane
	bound   bool
	exposed bool

	queue  []outEntry
	queued int
	// expected is the sequence the server sends next; delivered counts the items
	// handed to the reader; checkpoint is where the last delivered cursor
	// resumes from.
	expected           uint64
	delivered          uint64
	checkpoint         uint64
	checkpointTerminal bool
	terminal           bool
	ended              bool
	stopped            bool
	stopReason         string
	pendingCancel      string
	failure            error

	wake  chan struct{}
	space chan struct{}
}

type outEntry struct {
	first    uint64
	count    uint64
	value    schema.SchemaValue
	packed   []byte
	index    int
	cursor   string
	bytes    int
	terminal bool
	end      error
}

func newOutputStream(s *session, token string) *outputStream {
	return &outputStream{s: s, token: token, wake: make(chan struct{}, 1), space: make(chan struct{}, 1)}
}

// Release does nothing: the session owns the stream.
func (*outputStream) Release() {}

func (o *outputStream) expose() error {
	o.s.mu.Lock()
	defer o.s.mu.Unlock()
	if o.exposed {
		return ErrStreamConsumed
	}
	o.exposed = true
	return nil
}

// offer queues what the server sent, waiting while the queue is full.
func (o *outputStream) offer(c *sessionConn, e outEntry) error {
	s := o.s
	s.mu.Lock()
	for {
		if s.conn != c || s.over {
			s.mu.Unlock()
			return nil
		}
		if o.terminal {
			s.mu.Unlock()
			return protocolErrorf("a stream item arrived after the stream ended")
		}
		if e.first != o.expected {
			s.mu.Unlock()
			return protocolErrorf("stream sequence %d arrived where %d was expected", e.first, o.expected)
		}
		if o.stopped || e.terminal || len(o.queue) == 0 ||
			(len(o.queue) < maxQueuedItems && o.queued+e.bytes <= maxQueuedBytes && s.queued+e.bytes <= maxSessionQueued) {
			break
		}
		s.mu.Unlock()
		select {
		case <-o.space:
		case <-c.closed:
		case <-s.done:
		}
		s.mu.Lock()
	}
	defer s.mu.Unlock()
	o.expected = e.first + e.count
	if e.terminal {
		o.terminal = true
		o.pendingCancel = ""
	}
	if o.stopped {
		return nil
	}
	// A resumed stream replays from its last cursor; what the reader already
	// has is skipped, including the delivered part of a packed batch.
	if !e.terminal && e.first < o.delivered {
		skip := o.delivered - e.first
		if skip >= e.count {
			return nil
		}
		e.index = int(skip)
	}
	o.queue = append(o.queue, e)
	o.queued += e.bytes
	s.queued += e.bytes
	poke(o.wake)
	return nil
}

// pop removes the head entry. Called with s.mu held.
func (o *outputStream) pop() {
	e := o.queue[0]
	o.queue[0] = outEntry{}
	o.queue = o.queue[1:]
	o.queued -= e.bytes
	o.s.queued -= e.bytes
	poke(o.space)
}

// checkpointAt records a delivered position. Called with s.mu held.
func (o *outputStream) checkpointAt(next uint64, cursor string, terminal bool) {
	if cursor == "" {
		return
	}
	o.checkpoint = next
	o.checkpointTerminal = terminal
	o.s.cursors[o.token] = cursor
}

func (o *outputStream) next(ctx context.Context) (schema.SchemaValue, bool, error) {
	s := o.s
	s.mu.Lock()
	for {
		if len(o.queue) > 0 {
			e := &o.queue[0]
			if e.terminal {
				end := e.end
				o.checkpointAt(e.first, e.cursor, true)
				o.pop()
				o.ended = true
				s.mu.Unlock()
				return nil, false, end
			}
			var v schema.SchemaValue
			if e.packed != nil {
				v = schema.U8Value{Value: e.packed[e.index]}
				e.index++
				o.delivered = e.first + uint64(e.index)
				if e.index == len(e.packed) {
					o.checkpointAt(e.first+e.count, e.cursor, false)
					o.pop()
				}
			} else {
				v = e.value
				o.delivered = e.first + 1
				o.checkpointAt(e.first+1, e.cursor, false)
				o.pop()
			}
			s.mu.Unlock()
			return v, true, nil
		}
		switch {
		case o.ended || o.stopped:
			s.mu.Unlock()
			return nil, false, nil
		case o.failure != nil:
			err := o.failure
			s.mu.Unlock()
			return nil, false, err
		case s.over:
			err := s.err
			if err == nil {
				err = errors.New("golem: the invocation session ended before the stream did")
			}
			s.mu.Unlock()
			return nil, false, err
		}
		s.mu.Unlock()
		select {
		case <-o.wake:
		case <-s.done:
		case <-ctx.Done():
			return nil, false, ctx.Err()
		}
		s.mu.Lock()
	}
}

func (o *outputStream) ready() bool {
	o.s.mu.Lock()
	defer o.s.mu.Unlock()
	return len(o.queue) > 0 || o.ended || o.stopped || o.failure != nil || o.s.over
}

// stop is the reader closing the stream: the server is told unless the stream
// already ended.
func (o *outputStream) stop(reason string) {
	s := o.s
	s.mu.Lock()
	defer s.mu.Unlock()
	if o.ended || o.stopped {
		return
	}
	o.stopped = true
	o.stopReason = reason
	for len(o.queue) > 0 {
		o.pop()
	}
	if o.terminal || s.over {
		return
	}
	if ch, ok := s.tokenChannels[o.token]; ok && s.conn != nil {
		s.sendCancel(ch, reason)
	} else {
		o.pendingCancel = reason
	}
}

// resume prepares the stream for a new attachment: what was queued but not
// read is dropped, to be replayed from the last cursor. Called with s.mu held.
func (o *outputStream) resume() {
	for len(o.queue) > 0 {
		o.pop()
	}
	o.channel = 0
	o.expected = o.checkpoint
	o.terminal = o.checkpointTerminal
	if o.stopped && !o.terminal {
		o.pendingCancel = o.stopReason
	}
}

// --- values ----------------------------------------------------------------

// mapStreams rebuilds a value with each stream replaced by f's result.
func mapStreams(v schema.SchemaValue, f func(schema.StreamValue) (schema.SchemaValue, error)) (schema.SchemaValue, error) {
	each := func(items []schema.SchemaValue) ([]schema.SchemaValue, error) {
		out := make([]schema.SchemaValue, len(items))
		for i, item := range items {
			mapped, err := mapStreams(item, f)
			if err != nil {
				return nil, err
			}
			out[i] = mapped
		}
		return out, nil
	}
	optional := func(p *schema.SchemaValue) (*schema.SchemaValue, error) {
		if p == nil {
			return nil, nil
		}
		mapped, err := mapStreams(*p, f)
		return &mapped, err
	}
	switch n := v.(type) {
	case schema.StreamValue:
		return f(n)
	case schema.RecordValue:
		fields, err := each(n.Fields)
		return schema.RecordValue{Fields: fields}, err
	case schema.TupleValue:
		elements, err := each(n.Elements)
		return schema.TupleValue{Elements: elements}, err
	case schema.ListValue:
		items, err := each(n.Items)
		return schema.ListValue{Items: items}, err
	case schema.FixedListValue:
		items, err := each(n.Items)
		return schema.FixedListValue{Items: items}, err
	case schema.MapValue:
		entries := make([]schema.MapEntry, len(n.Entries))
		for i, entry := range n.Entries {
			key, err := mapStreams(entry.Key, f)
			if err != nil {
				return nil, err
			}
			value, err := mapStreams(entry.Value, f)
			if err != nil {
				return nil, err
			}
			entries[i] = schema.MapEntry{Key: key, Value: value}
		}
		return schema.MapValue{Entries: entries}, nil
	case schema.VariantValue:
		payload, err := optional(n.Payload)
		return schema.VariantValue{Case: n.Case, Payload: payload}, err
	case schema.OptionValue:
		inner, err := optional(n.Value)
		return schema.OptionValue{Value: inner}, err
	case schema.ResultValue:
		inner, err := optional(n.Value)
		return schema.ResultValue{IsErr: n.IsErr, Value: inner}, err
	case schema.UnionValue:
		body, err := mapStreams(n.Body, f)
		return schema.UnionValue{Tag: n.Tag, Body: body}, err
	}
	return v, nil
}

// mapTypedStreams is mapStreams guided by the value's type, so f learns each
// stream's item type. The value must already have validated against it.
func mapTypedStreams(
	ref schema.Ref, t schema.SchemaType, v schema.SchemaValue,
	f func(item *schema.SchemaType, sv schema.StreamValue) (schema.SchemaValue, error),
) (schema.SchemaValue, error) {
	resolved, err := ref.At(t).Resolved()
	if err != nil {
		return nil, err
	}
	walk := func(t schema.SchemaType, v schema.SchemaValue) (schema.SchemaValue, error) {
		return mapTypedStreams(ref, t, v, f)
	}
	optional := func(t *schema.SchemaType, p *schema.SchemaValue) (*schema.SchemaValue, error) {
		if t == nil || p == nil {
			return p, nil
		}
		mapped, err := walk(*t, *p)
		return &mapped, err
	}
	items := func(t schema.SchemaType, items []schema.SchemaValue) ([]schema.SchemaValue, error) {
		out := make([]schema.SchemaValue, len(items))
		for i, item := range items {
			mapped, err := walk(t, item)
			if err != nil {
				return nil, err
			}
			out[i] = mapped
		}
		return out, nil
	}
	switch b := resolved.Type().Body.(type) {
	case schema.StreamType:
		if sv, ok := v.(schema.StreamValue); ok {
			return f(b.Item, sv)
		}
	case schema.RecordType:
		if n, ok := v.(schema.RecordValue); ok && len(n.Fields) == len(b.Fields) {
			fields := make([]schema.SchemaValue, len(n.Fields))
			for i, field := range b.Fields {
				if fields[i], err = walk(field.Body, n.Fields[i]); err != nil {
					return nil, err
				}
			}
			return schema.RecordValue{Fields: fields}, nil
		}
	case schema.TupleType:
		if n, ok := v.(schema.TupleValue); ok && len(n.Elements) == len(b.Elements) {
			elements := make([]schema.SchemaValue, len(n.Elements))
			for i, element := range b.Elements {
				if elements[i], err = walk(element, n.Elements[i]); err != nil {
					return nil, err
				}
			}
			return schema.TupleValue{Elements: elements}, nil
		}
	case schema.ListType:
		if n, ok := v.(schema.ListValue); ok {
			mapped, err := items(b.Element, n.Items)
			return schema.ListValue{Items: mapped}, err
		}
	case schema.FixedListType:
		if n, ok := v.(schema.FixedListValue); ok {
			mapped, err := items(b.Element, n.Items)
			return schema.FixedListValue{Items: mapped}, err
		}
	case schema.MapType:
		if n, ok := v.(schema.MapValue); ok {
			entries := make([]schema.MapEntry, len(n.Entries))
			for i, entry := range n.Entries {
				key, err := walk(b.Key, entry.Key)
				if err != nil {
					return nil, err
				}
				value, err := walk(b.Value, entry.Value)
				if err != nil {
					return nil, err
				}
				entries[i] = schema.MapEntry{Key: key, Value: value}
			}
			return schema.MapValue{Entries: entries}, nil
		}
	case schema.VariantType:
		if n, ok := v.(schema.VariantValue); ok && int(n.Case) < len(b.Cases) {
			payload, err := optional(b.Cases[n.Case].Payload, n.Payload)
			return schema.VariantValue{Case: n.Case, Payload: payload}, err
		}
	case schema.OptionType:
		if n, ok := v.(schema.OptionValue); ok {
			inner, err := optional(&b.Inner, n.Value)
			return schema.OptionValue{Value: inner}, err
		}
	case schema.ResultType:
		if n, ok := v.(schema.ResultValue); ok {
			arm := b.Ok
			if n.IsErr {
				arm = b.Err
			}
			inner, err := optional(arm, n.Value)
			return schema.ResultValue{IsErr: n.IsErr, Value: inner}, err
		}
	case schema.UnionType:
		if n, ok := v.(schema.UnionValue); ok {
			for _, branch := range b.Branches {
				if branch.Tag == n.Tag {
					body, err := walk(branch.Body, n.Body)
					return schema.UnionValue{Tag: n.Tag, Body: body}, err
				}
			}
		}
	}
	return v, nil
}

// --- calls -----------------------------------------------------------------

// CallStreaming invokes a method that takes or returns a stream, over an
// invocation session, and decodes what it returns. It returns once the result
// arrives; streams in the result go on reading from the session. Cancelling
// ctx before then cancels the call and its streams.
func CallStreaming[T any](ctx context.Context, a *Agent, method string, params schema.SchemaValue, f Decoder[T]) (T, error) {
	var zero T
	s, err := openSession(a, method, params)
	if err != nil {
		return zero, err
	}
	r := s.await(ctx)
	if r.err != nil {
		return zero, r.err
	}
	if r.value == nil {
		err := fmt.Errorf("golem: %s returned no value", method)
		s.fail(err)
		return zero, err
	}
	out, err := f(r.value)
	if err != nil {
		err = fmt.Errorf("golem: decoding the result of %s: %w", method, err)
		s.fail(err)
		return zero, err
	}
	return out, nil
}

// InvokeStreaming invokes a method that takes a stream and returns nothing.
func InvokeStreaming(ctx context.Context, a *Agent, method string, params schema.SchemaValue) error {
	s, err := openSession(a, method, params)
	if err != nil {
		return err
	}
	return s.await(ctx).err
}
