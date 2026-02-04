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

import { PathSegment } from 'golem:agent/common';
import { rejectEmptyString } from './validation';

export function parsePath(path: string): PathSegment[] {
  if (!path.startsWith('/')) {
    throw new Error(`HTTP mount must start with "/"`);
  }

  const segments = path.split('/').slice(1);

  return segments.map((segment, index) => parseSegment(segment, index === segments.length - 1));
}

function parseSegment(segment: string, isLast: boolean): PathSegment {
  if (!segment) {
    throw new Error(`Empty path segment ("//") is not allowed`);
  }

  if (segment !== segment.trim()) {
    throw new Error(`Whitespace is not allowed in path segments`);
  }

  if (segment.startsWith('{') && segment.endsWith('}')) {
    const name = segment.slice(1, -1);

    if (!name) {
      throw new Error(`Empty path variable "{}" is not allowed`);
    }

    if (name.startsWith('*')) {
      if (!isLast) {
        throw new Error(
          `Remaining path variable "{${name}}" is only allowed as the last path segment`,
        );
      }

      const variableName = name.slice(1);
      rejectEmptyString(variableName, 'remaining path variable');

      return {
        tag: 'remaining-path-variable',
        val: { variableName },
      };
    }

    if (name === 'agent-type' || name === 'agent-version') {
      return {
        tag: 'system-variable',
        val: name,
      };
    }

    rejectEmptyString(name, 'path variable');

    return {
      tag: 'path-variable',
      val: { variableName: name },
    };
  }

  if (segment.includes('{') || segment.includes('}')) {
    throw new Error(
      `Path segment "${segment}" must be a whole variable like "{id}" and cannot mix literals and variables`,
    );
  }

  rejectEmptyString(segment, `Literal path segment cannot be an empty string`);

  return {
    tag: 'literal',
    val: segment,
  };
}
