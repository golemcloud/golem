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

//go:build !wasip1

package oplog

import (
	"errors"
	"iter"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// Off the wasm target there is no oplog to read; binding the generated calls
// here would drag their //go:wasmimport declarations into a native link.

var errNoHost = errors.New("golem: oplogs can only be read inside a component")

func Get(golem.AgentID, uint64) iter.Seq2[Entry, error] {
	return func(yield func(Entry, error) bool) { yield(Entry{}, errNoHost) }
}

func Search(golem.AgentID, string) iter.Seq2[SearchHit, error] {
	return func(yield func(SearchHit, error) bool) { yield(SearchHit{}, errNoHost) }
}
