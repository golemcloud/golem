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

package durablestreams

import (
	"encoding/json"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
	wire "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_durable_streams"
)

// Producer is a writer's identity and acknowledged progress.
type Producer struct {
	ID       string
	Epoch    uint64
	Sequence uint64
}

// AppendReceipt is the acknowledgement of an append.
type AppendReceipt struct {
	// NextOffset is absent from a duplicate acknowledgement.
	NextOffset golem.Option[string]
	Epoch      uint64
	Sequence   uint64
	Closed     bool
}

// WriteOptions configures a Writer; the zero value generates a durable
// producer id, uses epoch 0, a 30s attempt deadline and [DefaultRetry].
type WriteOptions struct {
	ProducerID golem.Option[string]
	Epoch      uint64
	Timeout    time.Duration
	Retry      RetryOptions
	// Auth authenticates the requests; its plaintext never reaches the agent.
	Auth golem.Option[golem.Secret[string]]
}

// appendCall performs one append attempt; the wasm build binds it to the host
// writer (durablestreams_wasm.go).
type appendCall func(wire.DurableStreamAppendRequest) (wire.DurableStreamAppendReceipt, *Error)

// maxProducerNumber is the largest epoch or sequence the protocol carries.
const maxProducerNumber = 1<<53 - 1

// Writer is a non-pipelining producer: one append is in flight at a time. A
// failed or interrupted append stays pending; resolve it with RetryPending
// before appending new data. Dropping a writer does not undo a write.
type Writer struct {
	append   appendCall
	producer Producer
	retry    RetryOptions
	pending  *pendingAppend
	closed   bool
}

type pendingAppend struct {
	request wire.DurableStreamAppendRequest
	failure uint32
	delay   time.Duration
}

// NewWriter creates a producer for the stream at url, whose messages have the
// given content type.
func NewWriter(url, contentType string, options WriteOptions) (*Writer, error) {
	if options.Epoch > maxProducerNumber {
		return nil, newError(KindInvalidRequest, "producer epoch exceeds 2^53-1")
	}
	if options.Timeout == 0 {
		options.Timeout = 30 * time.Second
	}
	if options.Retry == (RetryOptions{}) {
		options.Retry = DefaultRetry
	}
	id, ok := options.ProducerID.Get()
	if !ok {
		id = golem.GenerateIdempotencyKey().String()
	}
	return &Writer{
		append:   hostWriter(url, contentType, id, options),
		producer: Producer{ID: id, Epoch: options.Epoch},
		retry:    options.Retry,
	}, nil
}

// Producer returns the writer's identity and acknowledged progress.
func (w *Writer) Producer() Producer { return w.producer }

// HasPending reports an append that has not been acknowledged.
func (w *Writer) HasPending() bool { return w.pending != nil }

// AppendJSON appends values as separate JSON messages, closing the stream after
// them when close is set. An element that is an array stays one message.
func AppendJSON[T any](w *Writer, values []T, close bool) (AppendReceipt, error) {
	encoded := make([]string, 0, len(values))
	for _, v := range values {
		data, err := json.Marshal(v)
		if err != nil {
			return AppendReceipt{}, newError(KindInvalidRequest, "JSON encode failed: %v", err)
		}
		encoded = append(encoded, string(data))
	}
	if err := w.prepare(wire.MakeDurableStreamAppendPayloadJson(encoded), close); err != nil {
		return AppendReceipt{}, err
	}
	return w.RetryPending()
}

// AppendBytes appends bytes, closing the stream after them when close is set.
func (w *Writer) AppendBytes(data []byte, close bool) (AppendReceipt, error) {
	if err := w.prepare(wire.MakeDurableStreamAppendPayloadBytes(append([]byte(nil), data...)), close); err != nil {
		return AppendReceipt{}, err
	}
	return w.RetryPending()
}

// Close closes the stream; it uses a sequence number like an append.
func (w *Writer) Close() (AppendReceipt, error) {
	if err := w.prepare(wire.MakeDurableStreamAppendPayloadBytes(nil), true); err != nil {
		return AppendReceipt{}, err
	}
	return w.RetryPending()
}

func (w *Writer) prepare(payload wire.DurableStreamAppendPayload, close bool) error {
	switch {
	case w.pending != nil:
		return newError(KindSequenceConflict, "resolve the pending append before supplying new data")
	case w.closed:
		return newError(KindClosed, "writer is closed")
	case w.producer.Sequence > maxProducerNumber:
		return newError(KindInvalidRequest, "producer sequence exhausted")
	}
	empty := payload.Tag() == wire.DurableStreamAppendPayloadJson && len(payload.Json()) == 0 ||
		payload.Tag() == wire.DurableStreamAppendPayloadBytes && len(payload.Bytes()) == 0
	if empty && !close {
		return newError(KindInvalidRequest, "an empty append must close the stream")
	}
	w.pending = &pendingAppend{request: wire.DurableStreamAppendRequest{
		Payload: payload, Sequence: w.producer.Sequence, Close: close,
	}}
	return nil
}

// RetryPending sends the pending append again, retrying transient failures,
// and returns its acknowledgement.
func (w *Writer) RetryPending() (AppendReceipt, error) {
	for {
		p := w.pending
		if p == nil {
			return AppendReceipt{}, newError(KindInvalidRequest, "no pending append")
		}
		if p.delay > 0 {
			time.Sleep(p.delay)
			p.delay = 0
		}
		receipt, err := w.append(p.request)
		if err == nil {
			if err := w.acknowledge(receipt); err != nil {
				return AppendReceipt{}, err
			}
			out := AppendReceipt{NextOffset: golem.None[string](), Epoch: receipt.Epoch, Sequence: receipt.Sequence, Closed: receipt.Closed}
			if receipt.NextOffset.IsSome() {
				out.NextOffset = golem.Some(receipt.NextOffset.Some())
			}
			return out, nil
		}
		wait, retry := w.retry.delay(err, p.failure)
		if !retry {
			return AppendReceipt{}, err
		}
		p.failure++
		p.delay = wait
	}
}

func (w *Writer) acknowledge(receipt wire.DurableStreamAppendReceipt) error {
	request := w.pending.request
	switch {
	case receipt.Epoch != w.producer.Epoch || receipt.Sequence < request.Sequence:
		return newError(KindProtocolError, "invalid producer acknowledgement")
	case receipt.Sequence > request.Sequence:
		return newError(KindProducerDiverged, "peer acknowledged a later producer sequence")
	case request.Close && !receipt.Closed:
		return newError(KindProtocolError, "peer did not acknowledge closure")
	}
	w.producer.Sequence = request.Sequence + 1
	w.closed = receipt.Closed
	w.pending = nil
	return nil
}
