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

// Barrel for the fluent RDBMS surfaces. Re-exports the Postgres / MySql /
// Ignite entry points and their typed result/transaction/error shapes, plus
// the shared error + temporal-decode types.

export {
  Postgres,
  Pg,
  PostgresError,
  type PgConnection,
  type PgTransaction,
  type PgResultSet,
  type PgOpenOptions,
} from './postgres';

export {
  MySql,
  MySqlError,
  type MySqlConnection,
  type MySqlTransaction,
  type MySqlResultSet,
  type MySqlOpenOptions,
} from './mysql';

export {
  Ignite,
  IgniteError,
  type IgniteConnection,
  type IgniteTransaction,
  type IgniteResultSet,
  type IgniteOpenOptions,
  type IgniteUuid,
} from './ignite';

export { RdbmsError, type RdbmsErrorReason, type TemporalDecodeMode } from './shared';
