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

package durability

// oplogOps is the host API behind checkpoints. The wasm build binds the
// generated host calls (oplog_wasm.go); a native build has no host, so tests
// install their own.
type oplogOps interface {
	getOplogIndex() uint64
	setOplogIndex(index uint64)
}
