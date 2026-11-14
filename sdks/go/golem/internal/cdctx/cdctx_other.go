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

package cdctx

// Scope is one custom durable operation opened by Begin.
type Scope struct{}

// Begin opens an operation on the calling goroutine.
func Begin() *Scope { return nil }

// Entered records that the operation's live scope is installed.
func Entered(*Scope) {}

// Join waits until every goroutine started inside the operation has exited.
func Join(*Scope) {}

// End closes an operation opened by Begin.
func End(*Scope) {}
