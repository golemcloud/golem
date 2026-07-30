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

// A strict (fatal) UTF-8 TextDecoder that rejects invalid byte sequences,
// falling back to a lenient decoder on runtimes that don't support the `fatal`
// option. The QuickJS agent guest is compiled without ICU, where
// `new TextDecoder('utf-8', { fatal: true })` throws at construction — so any
// eager module-scope decoder must degrade gracefully there.
export function strictTextDecoder() {
  try {
    return new TextDecoder('utf-8', { fatal: true });
  } catch {
    return new TextDecoder('utf-8');
  }
}
