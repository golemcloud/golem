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

package oplog

import (
	"os"
	"regexp"
	"slices"
	"testing"

	oplogwit "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_oplog"
)

// TestEveryEntryCaseHasAConstant — cases.go aliases the generated case tags;
// regenerating the bindings with a new case must not leave it out.
func TestEveryEntryCaseHasAConstant(t *testing.T) {
	names := func(path string, re *regexp.Regexp) []string {
		t.Helper()
		src, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var out []string
		for _, m := range re.FindAllStringSubmatch(string(src), -1) {
			out = append(out, m[1])
		}
		slices.Sort(out)
		return out
	}
	generated := names("../internal/wit/golem_api_oplog/wit_bindings.go", regexp.MustCompile(`(?m)^\s*PublicOplogEntry(\w+)\s+uint8 = \d+`))
	aliased := names("cases.go", regexp.MustCompile(`(?m)^\s*(\w+) = oplogwit\.PublicOplogEntry\w+`))
	if len(generated) == 0 || !slices.Equal(generated, aliased) {
		t.Fatalf("cases.go is out of date with the bindings:\n generated %v\n aliased   %v", generated, aliased)
	}
}

func TestEntriesSwitchOnTheCaseConstants(t *testing.T) {
	e := oplogwit.MakePublicOplogEntryLog(LogParameters{Message: "hello"})
	switch e.Tag() {
	case AgentInvocationStarted, Error:
		t.Fatal("wrong case")
	case Log:
		if e.Log().Message != "hello" {
			t.Fatalf("message = %q", e.Log().Message)
		}
	default:
		t.Fatalf("unexpected tag %d", e.Tag())
	}
}
