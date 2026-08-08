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

// Package blobstore is a Go wrapper over Golem's durable object store
// (wasi:blobstore). Create or open a [Container], then read and write named blobs.
//
// Like [keyvalue], these are I/O operations that return an error (a missing
// object is reported as found=false, not an error), distinct from the fail-loud
// exactly-once control-flow surface. The store is durable, and because operations
// are remote side effects, calling them inside a read-only method traps.
//
// Pair a fallible call with golem.Must / golem.Must0 / golem.Must2 to abort the
// invocation on error.
package blobstore

import (
	"encoding/json"
	"fmt"
	"time"

	bstore "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_blobstore_blobstore"
	bscontainer "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_blobstore_container"
	bstypes "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_blobstore_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Error is a blobstore host error (the host reports errors as plain strings).
type Error struct{ Message string }

func (e *Error) Error() string { return "golem/blobstore: " + e.Message }

func bsError(msg string) error { return &Error{Message: msg} }

// ObjectID identifies an object by its container and name (for [CopyObject] /
// [MoveObject]).
type ObjectID struct {
	Container string
	Object    string
}

func (o ObjectID) toWit() bstypes.ObjectId {
	return bstypes.ObjectId{Container: o.Container, Object: o.Object}
}

// ContainerMetadata describes a container.
type ContainerMetadata struct {
	Name string
	// CreatedAt is the container's creation time (host millis). Some backends
	// report last-modified here rather than creation.
	CreatedAt time.Time
}

// ObjectMetadata describes an object.
type ObjectMetadata struct {
	Name      string
	Container string
	// CreatedAt is the object's creation time (host millis; see [ContainerMetadata]).
	CreatedAt time.Time
	Size      uint64
}

// ── Top-level container operations ────────────────────────────────────────────

// CreateContainer creates a new container.
func CreateContainer(name string) (*Container, error) {
	r := bstore.CreateContainer(name)
	if r.IsErr() {
		return nil, bsError(r.Err())
	}
	return &Container{raw: r.Ok(), name: name}, nil
}

// GetContainer opens an existing container.
func GetContainer(name string) (*Container, error) {
	r := bstore.GetContainer(name)
	if r.IsErr() {
		return nil, bsError(r.Err())
	}
	return &Container{raw: r.Ok(), name: name}, nil
}

// GetOrCreateContainer returns the container, creating it if it does not exist.
// It tolerates a concurrent create (the create/get race).
func GetOrCreateContainer(name string) (*Container, error) {
	exists, err := ContainerExists(name)
	if err != nil {
		return nil, err
	}
	if exists {
		return GetContainer(name)
	}
	c, err := CreateContainer(name)
	if err != nil {
		// Possibly created concurrently between the check and the create.
		if got, gerr := GetContainer(name); gerr == nil {
			return got, nil
		}
		return nil, err
	}
	return c, nil
}

// ContainerExists reports whether a container exists.
func ContainerExists(name string) (bool, error) {
	r := bstore.ContainerExists(name)
	if r.IsErr() {
		return false, bsError(r.Err())
	}
	return r.Ok(), nil
}

// DeleteContainer deletes a container and its objects.
func DeleteContainer(name string) error {
	if r := bstore.DeleteContainer(name); r.IsErr() {
		return bsError(r.Err())
	}
	return nil
}

// CopyObject copies an object from src to dest (across containers if they differ).
func CopyObject(src, dest ObjectID) error {
	if r := bstore.CopyObject(src.toWit(), dest.toWit()); r.IsErr() {
		return bsError(r.Err())
	}
	return nil
}

// MoveObject moves an object from src to dest.
func MoveObject(src, dest ObjectID) error {
	if r := bstore.MoveObject(src.toWit(), dest.toWit()); r.IsErr() {
		return bsError(r.Err())
	}
	return nil
}

// ── Container ─────────────────────────────────────────────────────────────────

// Container is a handle to an open blob container.
type Container struct {
	raw  *bscontainer.Container
	name string
}

// Name returns the container's name.
func (c *Container) Name() string { return c.name }

// Info returns the container's metadata.
func (c *Container) Info() (ContainerMetadata, error) {
	r := c.raw.Info()
	if r.IsErr() {
		return ContainerMetadata{}, bsError(r.Err())
	}
	m := r.Ok()
	return ContainerMetadata{Name: m.Name, CreatedAt: millis(m.CreatedAt)}, nil
}

// Has reports whether the named object exists.
func (c *Container) Has(name string) (bool, error) {
	r := c.raw.HasObject(name)
	if r.IsErr() {
		return false, bsError(r.Err())
	}
	return r.Ok(), nil
}

// ObjectInfo returns metadata for the named object (an error if it is absent).
func (c *Container) ObjectInfo(name string) (ObjectMetadata, error) {
	r := c.raw.ObjectInfo(name)
	if r.IsErr() {
		return ObjectMetadata{}, bsError(r.Err())
	}
	m := r.Ok()
	return ObjectMetadata{Name: m.Name, Container: m.Container, CreatedAt: millis(m.CreatedAt), Size: m.Size}, nil
}

// Delete removes the named object (a no-op if it is absent).
func (c *Container) Delete(name string) error {
	if r := c.raw.DeleteObject(name); r.IsErr() {
		return bsError(r.Err())
	}
	return nil
}

// DeleteMany removes several objects at once.
func (c *Container) DeleteMany(names []string) error {
	if r := c.raw.DeleteObjects(names); r.IsErr() {
		return bsError(r.Err())
	}
	return nil
}

// Clear removes all objects, leaving the container empty.
func (c *Container) Clear() error {
	if r := c.raw.Clear(); r.IsErr() {
		return bsError(r.Err())
	}
	return nil
}

// ListObjects returns all object names in the container. Order is not guaranteed.
func (c *Container) ListObjects() ([]string, error) {
	r := c.raw.ListObjects()
	if r.IsErr() {
		return nil, bsError(r.Err())
	}
	return drainStrings(r.Ok()), nil
}

// GetData reads the whole object's bytes. found is false (nil error) when the
// object is absent.
func (c *Container) GetData(name string) (data []byte, found bool, err error) {
	has, err := c.Has(name)
	if err != nil || !has {
		return nil, false, err
	}
	info, err := c.ObjectInfo(name)
	if err != nil {
		return nil, false, err
	}
	if info.Size == 0 {
		return []byte{}, true, nil
	}
	// The WIT spec says the range end is inclusive, but Golem's in-memory/fs
	// backends treat it as exclusive. Try inclusive first, then recover.
	first, err := c.getRange(name, 0, info.Size-1)
	if err != nil {
		return nil, false, err
	}
	if uint64(len(first)) == info.Size {
		return first, true, nil
	}
	rest, err := c.getRange(name, 0, info.Size)
	if err != nil {
		return nil, false, err
	}
	return rest, true, nil
}

// GetRange reads bytes [start, end] of an object. The host's inclusive/exclusive
// treatment of end differs across backends, so ranged reads are not portable —
// prefer [Container.GetData] for whole objects.
func (c *Container) GetRange(name string, start, end uint64) ([]byte, error) {
	return c.getRange(name, start, end)
}

func (c *Container) getRange(name string, start, end uint64) ([]byte, error) {
	r := c.raw.GetData(name, start, end)
	if r.IsErr() {
		return nil, bsError(r.Err())
	}
	cr := r.Ok().IncomingValueConsumeSync()
	if cr.IsErr() {
		return nil, bsError(cr.Err())
	}
	return cr.Ok(), nil
}

// WriteData creates or replaces the named object with data.
func (c *Container) WriteData(name string, data []byte) error {
	ov := bstypes.OutgoingValueNewOutgoingValue()
	writer, reader := bstypes.MakeStreamU8()
	if r := ov.OutgoingValueWriteBody(reader); r.IsErr() {
		return bsError("outgoing-value write-body failed")
	}
	// Feed the bytes into the stream and close it, then hand the outgoing value
	// to write-data (which reads the buffered stream).
	writer.WriteAll(data)
	writer.Drop()
	if r := c.raw.WriteData(name, ov); r.IsErr() {
		return bsError(r.Err())
	}
	return nil
}

func drainStrings(r *witTypes.StreamReader[string]) []string {
	var out []string
	buf := make([]string, 256)
	for {
		n := r.Read(buf)
		if n > 0 {
			out = append(out, buf[:n]...)
		}
		if r.WriterDropped() && n == 0 {
			break
		}
	}
	r.Drop()
	return out
}

func millis(ms uint64) time.Time { return time.UnixMilli(int64(ms)) }

// ── Typed store ───────────────────────────────────────────────────────────────

// Store is a typed view over a [Container]: object bodies are JSON-encoded T.
// Build one with [Typed].
type Store[T any] struct{ c *Container }

// Typed wraps a container as a JSON-encoded store of T.
func Typed[T any](c *Container) *Store[T] { return &Store[T]{c: c} }

// Get decodes the named object as T. found is false (nil error) when absent.
func (s *Store[T]) Get(name string) (value T, found bool, err error) {
	raw, found, err := s.c.GetData(name)
	if err != nil || !found {
		var zero T
		return zero, found, err
	}
	v, err := unmarshalValue[T](raw)
	if err != nil {
		var zero T
		return zero, true, fmt.Errorf("golem/blobstore: decoding %q: %w", name, err)
	}
	return v, true, nil
}

// Set JSON-encodes value and writes it as the named object.
func (s *Store[T]) Set(name string, value T) error {
	raw, err := marshalValue(value)
	if err != nil {
		return fmt.Errorf("golem/blobstore: encoding %q: %w", name, err)
	}
	return s.c.WriteData(name, raw)
}

// Delete removes the named object.
func (s *Store[T]) Delete(name string) error { return s.c.Delete(name) }

// Exists reports whether the named object is present.
func (s *Store[T]) Exists(name string) (bool, error) { return s.c.Has(name) }

// List returns the object names.
func (s *Store[T]) List() ([]string, error) { return s.c.ListObjects() }

// marshalValue/unmarshalValue are the pure JSON codec behind Store (no host
// calls), so they are natively testable.
func marshalValue[T any](v T) ([]byte, error) { return json.Marshal(v) }
func unmarshalValue[T any](data []byte) (T, error) {
	var v T
	err := json.Unmarshal(data, &v)
	return v, err
}
