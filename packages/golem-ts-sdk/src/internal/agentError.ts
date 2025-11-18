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

import { AgentError } from 'golem:agent/common';
import * as Value from './mapping/values/Value';

export function createCustomError(error: string): AgentError {
  return {
    tag: 'custom-error',
    val: {
      value: Value.toWitValue({
        kind: 'string',
        value: error,
      }),
      typ: {
        nodes: [
          {
            name: undefined,
            owner: undefined,
            type: {
              tag: 'prim-string-type',
            },
          },
        ],
      },
    },
  };
}

export function invalidMethod(error: string): AgentError {
  return {
    tag: 'invalid-method',
    val: error,
  };
}

export function invalidInput(error: string): AgentError {
  return {
    tag: 'invalid-input',
    val: error,
  };
}

export function invalidType(error: string): AgentError {
  return {
    tag: 'invalid-type',
    val: error,
  };
}

export function isAgentError(error: any): error is AgentError {
  return (
    error.tag !== undefined &&
    error.val !== undefined &&
    ((error.tag === 'invalid-input' && typeof error.val === 'string') ||
      (error.tag === 'invalid-method' && typeof error.val === 'string') ||
      (error.tag === 'invalid-type' && typeof error.val === 'string') ||
      (error.tag === 'invalid-agent-id' && typeof error.val === 'string') ||
      (error.tag === 'custom-error' &&
        typeof error.val === 'object' &&
        error.val.value !== undefined &&
        error.val.typ !== undefined))
  );
}
