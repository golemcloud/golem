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

package engine

import (
	"errors"
	"fmt"
	"reflect"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

// Values cross the boundary as a schema-value-tree: a flat pool of nodes plus
// the index of the root. This file holds only the node-level primitives; how a
// given Go type maps onto them lives in its codec (see codec.go).

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

type ValBuilder struct {
	Nodes []types.SchemaValueNode
	// restore gives back, newest first, the handles the encode moved out of
	// their owners.
	restore []func()
}

func (b *ValBuilder) Push(n types.SchemaValueNode) int32 {
	b.Nodes = append(b.Nodes, n)
	return int32(len(b.Nodes) - 1)
}

// Moved records how to give a handle the encode moved out of its owner back to
// it, should the encode fail before the host takes the tree.
func (b *ValBuilder) Moved(restore func()) { b.restore = append(b.restore, restore) }

// Restore gives every moved handle back to its owner, newest first, for a
// tree the host never took.
func (b *ValBuilder) Restore() {
	for i := len(b.restore) - 1; i >= 0; i-- {
		b.restore[i]()
	}
	b.restore = nil
}

// Restorer returns [ValBuilder.Restore] for a tree that outlives the builder
// and may still fail to reach the host.
func (b *ValBuilder) Restorer() func() {
	restore := b.restore
	b.restore = nil
	return func() {
		for i := len(restore) - 1; i >= 0; i-- {
			restore[i]()
		}
	}
}

// RestoreOnPanic gives every moved handle back to its owner and panics again
// when the encode panics. Defer it where the builder is created.
func (b *ValBuilder) RestoreOnPanic() {
	if r := recover(); r != nil {
		b.Restore()
		panic(r)
	}
}

// ErrPermissionCardTaken reports a permission card that already moved out of a
// value, by a decode or a send: the value can no longer be decoded or sent
// whole.
var ErrPermissionCardTaken = errors.New("golem: the permission card in this value was already decoded or sent")

// CheckCardsPresent returns [ErrPermissionCardTaken] when a permission card
// was decoded or sent out of tree. Check a tree that crosses the decode/send
// boundary — a received value sent on — before it is sent.
func CheckCardsPresent(tree types.SchemaValueTree) error {
	for _, n := range tree.ValueNodes {
		if n.Tag() == types.SchemaValueNodePermissionCardHandle && n.PermissionCardHandle() == nil {
			return ErrPermissionCardTaken
		}
	}
	return nil
}

// EncodeWith encodes v using c, producing a complete value tree. A failed
// encode gives every handle it moved back to its owner.
func EncodeWith(c *Codec, v reflect.Value) types.SchemaValueTree {
	var b ValBuilder
	defer b.RestoreOnPanic()
	root := c.Encode(&b, v)
	return types.SchemaValueTree{ValueNodes: b.Nodes, Root: root}
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

type Decoder struct {
	Nodes []types.SchemaValueNode
	// reads counts node reads. Each node of a tree has one parent, so a decode
	// reads it only a few times; a tree whose indices form a cycle would be
	// read forever, so too many reads end the decode instead.
	reads int
	// taken holds the nodes Take replaced, oldest first.
	taken []takenNode
}

type takenNode struct {
	idx  int32
	node types.SchemaValueNode
}

// Take replaces node idx with empty once the decode has moved the handle it
// held out of the tree, so that no second decode of the same tree can move it
// again. idx must have been read with Node.
func (d *Decoder) Take(idx int32, empty types.SchemaValueNode) {
	d.taken = append(d.taken, takenNode{idx: idx, node: d.Nodes[idx]})
	d.Nodes[idx] = empty
}

// Restore puts back every node Take replaced, for a decode that failed and
// whose result is discarded.
func (d *Decoder) Restore() {
	for i := len(d.taken) - 1; i >= 0; i-- {
		d.Nodes[d.taken[i].idx] = d.taken[i].node
	}
	d.taken = nil
}

// ReleaseCards gives every permission card in the tree back to the host with
// release, both those the decode adopted and those it never reached, for a
// decode of a received tree that failed and whose result is discarded.
func (d *Decoder) ReleaseCards(release func(*types.PermissionCard)) {
	d.Restore()
	ReleaseCardsIn(d.Nodes, release)
}

// ReleaseRest gives back the permission cards a successful decode of a
// received tree did not reach, such as those in values past the ones it read.
// The cards it adopted stay with the result.
func (d *Decoder) ReleaseRest(release func(*types.PermissionCard)) {
	ReleaseCardsIn(d.Nodes, release)
}

// ReleaseCardsIn gives every permission card still in nodes back to the host
// with release, and marks its node taken.
func ReleaseCardsIn(nodes []types.SchemaValueNode, release func(*types.PermissionCard)) {
	for i, n := range nodes {
		if n.Tag() == types.SchemaValueNodePermissionCardHandle && n.PermissionCardHandle() != nil {
			release(n.PermissionCardHandle())
			nodes[i] = types.MakeSchemaValueNodePermissionCardHandle(nil)
		}
	}
}

// MarkCardsSent marks every permission-card node in nodes taken once the host
// has taken the tree, so a second send of the same value fails with
// [ErrPermissionCardTaken] instead of naming handles the host already owns.
func MarkCardsSent(nodes []types.SchemaValueNode) {
	for i, n := range nodes {
		if n.Tag() == types.SchemaValueNodePermissionCardHandle {
			nodes[i] = types.MakeSchemaValueNodePermissionCardHandle(nil)
		}
	}
}

// maxReadsPerNode bounds how often a decode may read each node on average.
const maxReadsPerNode = 8

func (d *Decoder) Node(idx int32) (types.SchemaValueNode, error) {
	if idx < 0 || int(idx) >= len(d.Nodes) {
		return types.SchemaValueNode{}, fmt.Errorf("value node index %d out of range (%d nodes)", idx, len(d.Nodes))
	}
	d.reads++
	if d.reads > maxReadsPerNode*len(d.Nodes)+maxReadsPerNode {
		return types.SchemaValueNode{}, fmt.Errorf("value tree of %d nodes refers back to itself", len(d.Nodes))
	}
	return d.Nodes[idx], nil
}
