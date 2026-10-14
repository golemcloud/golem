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

package link

import "testing"

// A panic inside a region that must not resume reaches the trap, which on the
// wasm target ends the component, so no caller can recover it and go on.
func TestTrapOnPanicTrapsInsteadOfUnwinding(t *testing.T) {
	prev := TrapPanic
	t.Cleanup(func() { TrapPanic = prev })
	var region string
	var value any
	TrapPanic = func(r string, v any) { region, value = r, v }

	ran := false
	TrapOnPanic("atomic region", func() { ran = true })
	if !ran || region != "" {
		t.Fatalf("a body that returns normally ran=%v, trapped %q", ran, region)
	}

	TrapOnPanic("atomic region", func() { panic("disk gone") })
	if region != "atomic region" || value != "disk gone" {
		t.Fatalf("trapped %q with %v", region, value)
	}
}
