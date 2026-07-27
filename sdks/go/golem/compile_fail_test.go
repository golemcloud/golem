// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem_test

import (
	"os"
	"os/exec"
	"strings"
	"testing"
)

// The cross-agent type-safety guarantee is only useful if it actually fails to
// compile. This drives `go build` over a testdata program that Calls a
// PaymentID method with an OrderID client and asserts the type-checker rejects
// it — turning "it's a compile error" from a comment into a test.
func TestWrongAgentClientIsACompileError(t *testing.T) {
	goBin, err := exec.LookPath("go")
	if err != nil {
		t.Skip("go toolchain not on PATH; skipping compile-failure check")
	}

	cmd := exec.Command(goBin, "build", "-o", os.DevNull, "./testdata/wrong_client.go")
	out, err := cmd.CombinedOutput()

	if err == nil {
		t.Fatalf("expected ./testdata/wrong_client.go to fail compilation, but it built successfully")
	}

	msg := string(out)
	// The error must be the client-type mismatch, not some unrelated failure
	// (e.g. a link error would name a relocation, not these types).
	for _, want := range []string{"Client[OrderID]", "Client[PaymentID]", "Charge.Call"} {
		if !strings.Contains(msg, want) {
			t.Fatalf("compile error did not mention %q; got:\n%s", want, msg)
		}
	}
}
