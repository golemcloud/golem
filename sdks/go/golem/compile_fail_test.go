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

package golem_test

import (
	"os"
	"os/exec"
	"strings"
	"testing"
)

// TestWrongAgentClientIsACompileError — The cross-agent type-safety guarantee is only useful if it actually fails to
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

// TestUserCodeCannotBuildOrConvertAPermissionCard drives `go build` over a
// testdata program that tries to build a card from a CardID, from a struct
// literal and from a struct of the card's shape, to convert between the card
// types, to attach a card other than a ScopeCard to a call, to send, install
// or revoke a ScopeCard, and to convert or build a WalletCard, and asserts the
// type-checker rejects each.
func TestUserCodeCannotBuildOrConvertAPermissionCard(t *testing.T) {
	goBin, err := exec.LookPath("go")
	if err != nil {
		t.Skip("go toolchain not on PATH; skipping compile-failure check")
	}

	// -e reports every error rather than stopping after ten.
	out, err := exec.Command(goBin, "build", "-gcflags=-e", "-o", os.DevNull, "./testdata/forge_card.go").CombinedOutput()
	if err == nil {
		t.Fatalf("expected ./testdata/forge_card.go to fail compilation, but it built successfully")
	}
	msg := string(out)
	want := []string{
		"forge_card.go:18:27: cannot convert id",
		"forge_card.go:20:27: cannot refer to unexported field st",
		"forge_card.go:23:38: cannot convert lookalike",
		"forge_card.go:28:38: cannot convert card",
		"forge_card.go:29:27: cannot convert poly",
		"forge_card.go:30:22: cannot convert card",
		"forge_card.go:31:27: cannot convert scope",
		"forge_card.go:35:22: cannot use card (variable of struct type golem.PermissionCard) as golem.ScopeCard value",
		"forge_card.go:36:22: cannot use poly (variable of struct type golem.PolymorphicPermissionCard) as golem.ScopeCard value",
		"forge_card.go:37:22: cannot use golem.Wallet()[0] (variable of struct type golem.WalletCard) as golem.ScopeCard value",
		"forge_card.go:39:22: cannot use card (variable of struct type golem.PermissionCard) as golem.ScopeCard value",
		"forge_card.go:40:31: cannot use poly (variable of struct type golem.PolymorphicPermissionCard) as golem.ScopeCard value",
		"forge_card.go:42:15: scope.Revoke undefined",
		"forge_card.go:43:12: scope.Install undefined",
		"forge_card.go:44:34: cannot use scope",
		"forge_card.go:47:27: cannot convert w",
		"forge_card.go:48:38: cannot convert w",
		"forge_card.go:49:22: cannot convert w",
		"forge_card.go:50:23: cannot refer to unexported field kind",
		"forge_card.go:51:34: cannot use w",
	}
	for _, w := range want {
		if !strings.Contains(msg, w) {
			t.Fatalf("compile error did not mention %q; got:\n%s", w, msg)
		}
	}
	if got := strings.Count(msg, "forge_card.go:"); got != len(want) {
		t.Fatalf("expected %d errors, got %d:\n%s", len(want), got, msg)
	}
}
