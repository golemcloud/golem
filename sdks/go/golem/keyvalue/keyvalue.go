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

// Package keyvalue is a Go wrapper over Golem's durable key-value store
// (wasi:keyvalue). Open a [Bucket] by name and Get/Set/Delete/Exists byte values,
// or wrap it with [Typed] for a JSON-encoded [Store] of a Go type.
//
// The bucket operations return no error: the host fails one only on a denied
// permission or on a backend failure it has already retried, and neither is
// something the calling agent can recover from, so such a failure panics and
// fails the invocation. A missing key is not a failure: Get returns
// (nil, false). The store is durable — operations are journaled and replayed —
// but because they are remote side effects, calling them inside a read-only
// method traps.
package keyvalue

import (
	"encoding/json"
	"fmt"

	eventual "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_keyvalue_eventual"
	batch "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_keyvalue_eventual_batch"
	kvtypes "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_keyvalue_types"
	kverr "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_keyvalue_wasi_keyvalue_error"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

func hostFailure(op string, e *kverr.Error) error {
	if e == nil {
		return fmt.Errorf("golem/keyvalue: %s: unknown error", op)
	}
	return fmt.Errorf("golem/keyvalue: %s: %s", op, e.Trace())
}

// Bucket is a handle to a key-value bucket.
type Bucket struct{ raw *kvtypes.Bucket }

// OpenBucket opens (or creates) the named bucket.
func OpenBucket(name string) *Bucket {
	r := kvtypes.BucketOpenBucket(name)
	if r.IsErr() {
		panic(hostFailure("open bucket "+name, r.Err()))
	}
	return &Bucket{raw: r.Ok()}
}

// Get returns the value stored at key. found is false when the key is absent.
func (b *Bucket) Get(key string) (value []byte, found bool) {
	r := eventual.Get(b.raw, key)
	if r.IsErr() {
		panic(hostFailure("get", r.Err()))
	}
	opt := r.Ok()
	if opt.IsNone() {
		return nil, false
	}
	return consume(opt.Some()), true
}

// Set stores value at key.
func (b *Bucket) Set(key string, value []byte) {
	if r := eventual.Set(b.raw, key, outgoing(value)); r.IsErr() {
		panic(hostFailure("set", r.Err()))
	}
}

// Delete removes key (a no-op if it is absent).
func (b *Bucket) Delete(key string) {
	if r := eventual.Delete(b.raw, key); r.IsErr() {
		panic(hostFailure("delete", r.Err()))
	}
}

// Exists reports whether key is present.
func (b *Bucket) Exists(key string) bool {
	r := eventual.Exists(b.raw, key)
	if r.IsErr() {
		panic(hostFailure("exists", r.Err()))
	}
	return r.Ok()
}

// Keys lists all keys in the bucket.
func (b *Bucket) Keys() []string {
	r := batch.Keys(b.raw)
	if r.IsErr() {
		panic(hostFailure("keys", r.Err()))
	}
	return r.Ok()
}

// GetMany fetches several keys at once, returning a map of only the present keys.
// The batch is not atomic.
func (b *Bucket) GetMany(keys []string) map[string][]byte {
	r := batch.GetMany(b.raw, keys)
	if r.IsErr() {
		panic(hostFailure("get many", r.Err()))
	}
	opts := r.Ok()
	out := make(map[string][]byte, len(opts))
	for i, o := range opts {
		if i >= len(keys) || o.IsNone() {
			continue
		}
		out[keys[i]] = consume(o.Some())
	}
	return out
}

// SetMany stores several entries at once. The batch is not atomic.
func (b *Bucket) SetMany(entries map[string][]byte) {
	kvs := make([]witTypes.Tuple2[string, *kvtypes.OutgoingValue], 0, len(entries))
	for k, v := range entries {
		kvs = append(kvs, witTypes.Tuple2[string, *kvtypes.OutgoingValue]{F0: k, F1: outgoing(v)})
	}
	if r := batch.SetMany(b.raw, kvs); r.IsErr() {
		panic(hostFailure("set many", r.Err()))
	}
}

// DeleteMany removes several keys at once. The batch is not atomic.
func (b *Bucket) DeleteMany(keys []string) {
	if r := batch.DeleteMany(b.raw, keys); r.IsErr() {
		panic(hostFailure("delete many", r.Err()))
	}
}

func consume(iv *kvtypes.IncomingValue) []byte {
	r := iv.IncomingValueConsumeSync()
	if r.IsErr() {
		panic(hostFailure("read value", r.Err()))
	}
	return r.Ok()
}

func outgoing(value []byte) *kvtypes.OutgoingValue {
	ov := kvtypes.OutgoingValueNewOutgoingValue()
	if r := ov.OutgoingValueWriteBodySync(value); r.IsErr() {
		panic(hostFailure("write value", r.Err()))
	}
	return ov
}

// ── Typed store ───────────────────────────────────────────────────────────────

// Store is a typed view over a [Bucket]: values are JSON-encoded T. Build one
// with [Bucket.Typed].
type Store[T any] struct{ b *Bucket }

// Typed returns a view of the bucket that encodes and decodes values as T with
// encoding/json; a []byte value is stored as a base64 JSON string.
//
//	cart, found := bucket.Typed[Cart]().MustGet("cart-1")
func (b *Bucket) Typed[T any]() *Store[T] { return &Store[T]{b: b} }

// Typed is [Bucket.Typed] as a free function, for call sites that read better
// with the type first.
func Typed[T any](b *Bucket) *Store[T] { return b.Typed[T]() }

// Get decodes the value at key as T. found is false (nil error) when absent; the
// error reports stored data that does not decode as T.
func (s *Store[T]) Get(key string) (value T, found bool, err error) {
	raw, found := s.b.Get(key)
	if !found {
		var zero T
		return zero, false, nil
	}
	v, err := unmarshalValue[T](raw)
	if err != nil {
		var zero T
		return zero, true, fmt.Errorf("golem/keyvalue: decoding %q: %w", key, err)
	}
	return v, true, nil
}

// MustGet is [Store.Get] that panics when the stored data does not decode as T.
func (s *Store[T]) MustGet(key string) (value T, found bool) {
	v, found, err := s.Get(key)
	if err != nil {
		panic(err)
	}
	return v, found
}

// Set JSON-encodes value and stores it at key. It panics when value does not
// encode as JSON.
func (s *Store[T]) Set(key string, value T) {
	raw, err := marshalValue(value)
	if err != nil {
		panic(fmt.Errorf("golem/keyvalue: encoding %q: %w", key, err))
	}
	s.b.Set(key, raw)
}

// Delete removes key.
func (s *Store[T]) Delete(key string) { s.b.Delete(key) }

// Exists reports whether key is present.
func (s *Store[T]) Exists(key string) bool { return s.b.Exists(key) }

// Keys lists all keys.
func (s *Store[T]) Keys() []string { return s.b.Keys() }

// marshalValue/unmarshalValue are the JSON codec behind Store.
func marshalValue[T any](v T) ([]byte, error) { return json.Marshal(v) }
func unmarshalValue[T any](data []byte) (T, error) {
	var v T
	err := json.Unmarshal(data, &v)
	return v, err
}
