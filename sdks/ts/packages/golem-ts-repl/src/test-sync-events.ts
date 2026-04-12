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

import fs from 'node:fs';

export type TestSyncEvent = 'repl_ready' | 'eval_done' | 'completion_done';

let nextSeq = 1;
let writeEvent: (event: TestSyncEvent) => void = () => {};

export function initTestSyncEventsFromEnv(): void {
  const testSyncEventsFilePath = process.env.GOLEM_TS_REPL_TEST_SYNC_EVENTS_FILE;
  if (!testSyncEventsFilePath) {
    return;
  }

  writeEvent = (event: TestSyncEvent): void => {
    const line = JSON.stringify({ seq: nextSeq++, event }) + '\n';
    fs.appendFileSync(testSyncEventsFilePath, line, 'utf8');
  };
}

export function writeTestSyncEvent(event: TestSyncEvent): void {
  writeEvent(event);
}
