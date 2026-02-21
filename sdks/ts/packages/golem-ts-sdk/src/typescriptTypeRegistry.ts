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

// A wrapper over golem-ts-types-core/TypeMetadata to be used from user's code
// for them to register its types with the SDk.

import { TypeMetadata } from '@golemcloud/golem-ts-types-core';

export const TypescriptTypeRegistry = {
  register(typeMetadata: any): void {
    TypeMetadata.loadFromJson(typeMetadata);
  },
};
