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
// Unlike the exactly-once control-flow surface (RPC, promises, durability,
// transactions), which is fail-loud, these are I/O operations whose failures a
// caller can handle, so they return an error. A missing key is not an error: Get
// returns (nil, false, nil). The store is durable — operations are journaled and
// replayed — but because they are remote side effects, calling them inside a
// read-only method traps.
//
// Pair a fallible call with golem.Must / golem.Must0 / golem.Must2 to abort the
// invocation on error.
package keyvalue

import (
	"encoding/json"
	"errors"
	"fmt"

	eventual "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_keyvalue_eventual"
	batch "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_keyvalue_eventual_batch"
	kvtypes "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_keyvalue_types"
	kverr "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_keyvalue_wasi_keyvalue_error"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Error is a key-value host error, carrying the host's diagnostic trace.
type Error struct{ Trace string }

func (e *Error) Error() string { return "golem/keyvalue: " + e.Trace }

func kvError(e *kverr.Error) error {
	if e == nil {
		return errors.New("golem/keyvalue: unknown error")
	}
	return &Error{Trace: e.Trace()}
}

// Bucket is a handle to a key-value bucket.
type Bucket struct{ raw *kvtypes.Bucket }

// OpenBucket opens (or creates) the named bucket.
func OpenBucket(name string) (*Bucket, error) {
	r := kvtypes.BucketOpenBucket(name)
	if r.IsErr() {
		return nil, kvError(r.Err())
	}
	return &Bucket{raw: r.Ok()}, nil
}

// Get returns the value stored at key. found is false (with a nil error) when the
// key is absent.
func (b *Bucket) Get(key string) (value []byte, found bool, err error) {
	r := eventual.Get(b.raw, key)
	if r.IsErr() {
		return nil, false, kvError(r.Err())
	}
	opt := r.Ok()
	if opt.IsNone() {
		return nil, false, nil
	}
	return consume(opt.Some())
}

// Set stores value at key.
func (b *Bucket) Set(key string, value []byte) error {
	ov, err := outgoing(value)
	if err != nil {
		return err
	}
	if r := eventual.Set(b.raw, key, ov); r.IsErr() {
		return kvError(r.Err())
	}
	return nil
}

// Delete removes key (a no-op if it is absent).
func (b *Bucket) Delete(key string) error {
	if r := eventual.Delete(b.raw, key); r.IsErr() {
		return kvError(r.Err())
	}
	return nil
}

// Exists reports whether key is present.
func (b *Bucket) Exists(key string) (bool, error) {
	r := eventual.Exists(b.raw, key)
	if r.IsErr() {
		return false, kvError(r.Err())
	}
	return r.Ok(), nil
}

// Keys lists all keys in the bucket.
func (b *Bucket) Keys() ([]string, error) {
	r := batch.Keys(b.raw)
	if r.IsErr() {
		return nil, kvError(r.Err())
	}
	return r.Ok(), nil
}

// GetMany fetches several keys at once, returning a map of only the present keys.
// The batch is not atomic.
func (b *Bucket) GetMany(keys []string) (map[string][]byte, error) {
	r := batch.GetMany(b.raw, keys)
	if r.IsErr() {
		return nil, kvError(r.Err())
	}
	opts := r.Ok()
	out := make(map[string][]byte, len(opts))
	for i, o := range opts {
		if i >= len(keys) || o.IsNone() {
			continue
		}
		v, _, err := consume(o.Some())
		if err != nil {
			return nil, err
		}
		out[keys[i]] = v
	}
	return out, nil
}

// SetMany stores several entries at once. The batch is not atomic.
func (b *Bucket) SetMany(entries map[string][]byte) error {
	kvs := make([]witTypes.Tuple2[string, *kvtypes.OutgoingValue], 0, len(entries))
	for k, v := range entries {
		ov, err := outgoing(v)
		if err != nil {
			return err
		}
		kvs = append(kvs, witTypes.Tuple2[string, *kvtypes.OutgoingValue]{F0: k, F1: ov})
	}
	if r := batch.SetMany(b.raw, kvs); r.IsErr() {
		return kvError(r.Err())
	}
	return nil
}

// DeleteMany removes several keys at once. The batch is not atomic.
func (b *Bucket) DeleteMany(keys []string) error {
	if r := batch.DeleteMany(b.raw, keys); r.IsErr() {
		return kvError(r.Err())
	}
	return nil
}

func consume(iv *kvtypes.IncomingValue) ([]byte, bool, error) {
	r := iv.IncomingValueConsumeSync()
	if r.IsErr() {
		return nil, false, kvError(r.Err())
	}
	return r.Ok(), true, nil
}

func outgoing(value []byte) (*kvtypes.OutgoingValue, error) {
	ov := kvtypes.OutgoingValueNewOutgoingValue()
	if r := ov.OutgoingValueWriteBodySync(value); r.IsErr() {
		return nil, kvError(r.Err())
	}
	return ov, nil
}

// ── Typed store ───────────────────────────────────────────────────────────────

// Store is a typed view over a [Bucket]: values are JSON-encoded T. Build one with
// [Typed].
type Store[T any] struct{ b *Bucket }

// Typed wraps a bucket as a JSON-encoded store of T.
func Typed[T any](b *Bucket) *Store[T] { return &Store[T]{b: b} }

// Get decodes the value at key as T. found is false (nil error) when absent.
func (s *Store[T]) Get(key string) (value T, found bool, err error) {
	raw, found, err := s.b.Get(key)
	if err != nil || !found {
		var zero T
		return zero, found, err
	}
	v, err := unmarshalValue[T](raw)
	if err != nil {
		var zero T
		return zero, true, fmt.Errorf("golem/keyvalue: decoding %q: %w", key, err)
	}
	return v, true, nil
}

// Set JSON-encodes value and stores it at key.
func (s *Store[T]) Set(key string, value T) error {
	raw, err := marshalValue(value)
	if err != nil {
		return fmt.Errorf("golem/keyvalue: encoding %q: %w", key, err)
	}
	return s.b.Set(key, raw)
}

// Delete removes key.
func (s *Store[T]) Delete(key string) error { return s.b.Delete(key) }

// Exists reports whether key is present.
func (s *Store[T]) Exists(key string) (bool, error) { return s.b.Exists(key) }

// Keys lists all keys.
func (s *Store[T]) Keys() ([]string, error) { return s.b.Keys() }

// marshalValue/unmarshalValue are the pure JSON codec used by Store (no host
// calls), so they are natively testable.
func marshalValue[T any](v T) ([]byte, error) { return json.Marshal(v) }
func unmarshalValue[T any](data []byte) (T, error) {
	var v T
	err := json.Unmarshal(data, &v)
	return v, err
}
