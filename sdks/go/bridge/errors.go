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

import "fmt"

// Error is a call that did not produce a result. Endpoint always says which
// call it was; Status and Body are set only when the server answered, so a
// caller can tell a rejected request from one that never arrived.
type Error struct {
	Endpoint string
	Status   int
	Body     string
	Err      error
}

func (e *Error) Error() string {
	switch {
	case e.Status != 0 && e.Body != "":
		return fmt.Sprintf("golem: %s failed with HTTP %d: %s", e.Endpoint, e.Status, e.Body)
	case e.Status != 0:
		return fmt.Sprintf("golem: %s failed with HTTP %d", e.Endpoint, e.Status)
	default:
		return fmt.Sprintf("golem: %s failed: %v", e.Endpoint, e.Err)
	}
}

func (e *Error) Unwrap() error { return e.Err }
