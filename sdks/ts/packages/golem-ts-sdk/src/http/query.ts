// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import { QueryVariable } from 'golem:agent/common';
import { rejectEmpty } from './validation';

export function parseQuery(query: string): QueryVariable[] {
  if (!query) return [];

  return query.split('&').map((pair) => {
    const [key, value] = pair.split('=');

    if (!key || !value) {
      throw new Error(`Invalid query segment "${pair}"`);
    }

    if (!value.startsWith('{') || !value.endsWith('}')) {
      throw new Error(`Query value for "${key}" must be a variable reference`);
    }

    const trimmedValue = value.trim();

    const variableName = trimmedValue.slice(1, -1);
    rejectEmpty(variableName);

    return {
      queryParamName: key,
      variableName,
    };
  });
}
