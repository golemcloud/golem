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

package tool

import (
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"strings"
	"testing"
)

// mustDefErr asserts that a definition error mentioning want was recorded.
func mustDefErr(t *testing.T, d *definitions, want string) {
	t.Helper()
	for _, e := range d.Errs {
		if strings.Contains(e.Error(), want) {
			return
		}
	}
	t.Fatalf("expected a definition error mentioning %q; got %v", want, d.Errs)
}

func containsDefErr(errs []engine.DefError, want string) bool {
	for _, e := range errs {
		if strings.Contains(e.Error(), want) {
			return true
		}
	}
	return false
}
