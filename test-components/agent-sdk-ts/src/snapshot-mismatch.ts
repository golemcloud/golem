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

// An agent whose custom snapshot load does not reproduce its live state: `approx` is a running
// sum of the added string lengths while live, but `save` leaves it out and `load` derives it from
// the length of the saved items' JSON. A replay that starts from a snapshot of this agent returns
// a different `add` result than the one recorded, and diverges.

import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const SnapshotMismatchAgent = defineAgent({
  name: 'SnapshotMismatchAgent',
  id: { name: z.string() },
  snapshotting: { everyNInvocations: 2 },
  methods: {
    add: method({ input: { s: z.string() }, returns: z.number() }),
  },
});

export const SnapshotMismatchAgentImpl = SnapshotMismatchAgent.implement({
  init: () => ({ items: [] as string[], approx: 0 }),
  methods: {
    add({ s }) {
      this.items.push(s);
      this.approx += s.length;
      return this.approx;
    },
  },
  snapshot: {
    save() {
      return new TextEncoder().encode(JSON.stringify(this.items));
    },
    load(bytes, _context) {
      const json = new TextDecoder().decode(bytes);
      const items = JSON.parse(json) as string[];
      return { items, approx: json.length };
    },
  },
});
