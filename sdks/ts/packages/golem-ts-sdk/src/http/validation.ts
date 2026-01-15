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

export function rejectEmptyString(name: string, entityName: string) {
  if (name.length === 0) {
    throw new Error(`HTTP ${entityName} must not be empty`);
  }
}

export function rejectQueryParamsInPath(path: string, entityName: string) {
  if (path.includes('?')) {
    throw new Error(`HTTP ${entityName} must not contain query parameters`);
  }
}
