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

package golem

import (
	"testing"

	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
)

// TestPersistenceLevelRoundTrip — each level survives conversion to the host wire
// form and back, and maps to the expected host tag.
func TestPersistenceLevelRoundTrip(t *testing.T) {
	cases := []struct {
		level PersistenceLevel
		tag   uint8
	}{
		{PersistNothing, apiHost.PersistenceLevelPersistNothing},
		{PersistRemoteSideEffects, apiHost.PersistenceLevelPersistRemoteSideEffects},
		{PersistSmart, apiHost.PersistenceLevelSmart},
	}
	for _, c := range cases {
		w := c.level.toWit()
		if w.Tag() != c.tag {
			t.Fatalf("%v toWit tag = %d, want %d", c.level, w.Tag(), c.tag)
		}
		if got := persistenceLevelFromWit(w); got != c.level {
			t.Fatalf("round-trip = %v, want %v", got, c.level)
		}
	}
}
