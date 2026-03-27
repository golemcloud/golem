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

// Re-exports from the wasm-rquickjs `node:sqlite` builtin.
// The SDK imports SQLite functionality through this module so that
// tests can mock it in a single place instead of intercepting `node:sqlite`.
//
// Type declarations for wasm-rquickjs extensions are in types/node-sqlite-extensions.d.ts

export {
  DatabaseSync,
  StatementSync,
  Session,
  SQLTagStore,
  serializeDatabaseSync,
  restoreDatabaseSync,
  isAutocommitDatabaseSync,
} from 'node:sqlite';
