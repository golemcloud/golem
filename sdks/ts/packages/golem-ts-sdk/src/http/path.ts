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

import { PathSegment, PathSegmentNode } from 'golem:agent/common';
import { rejectEmpty } from './validation';

export function parsePath(path: string): PathSegment[] {
  if (!path.startsWith('/')) {
    throw new Error(`HTTP mount must start with "/"`);
  }

  const segments = path.split('/').slice(1);

  return segments.map(parseSegment);
}

function parseSegment(segment: string): PathSegment {
  if (!segment) {
    throw new Error(`Empty path segment ("//") is not allowed`);
  }

  const trimmedSegment = segment.trim();

  const nodes: PathSegmentNode[] = [];

  let i = 0;
  while (i < trimmedSegment.length) {
    if (trimmedSegment[i] === '{') {
      const end = trimmedSegment.indexOf('}', i);
      if (end === -1) {
        throw new Error(`Unclosed "{" in path segment "${segment}"`);
      }

      const name = trimmedSegment.slice(i + 1, end);

      if (!name) {
        throw new Error(`Empty path variable "{}" is not allowed`);
      }

      if (name === 'agent-type' || name === 'agent-version') {
        nodes.push({
          tag: 'system-variable',
          val: name,
        });
      } else {
        rejectEmpty(name);
        nodes.push({
          tag: 'path-variable',
          val: { variableName: name },
        });
      }

      i = end + 1;
      continue;
    }

    let start = i;
    while (i < trimmedSegment.length && trimmedSegment[i] !== '{') {
      i++;
    }

    const literal = trimmedSegment.slice(start, i);
    rejectEmpty(literal);

    nodes.push({
      tag: 'literal',
      val: literal,
    });
  }

  return { concat: nodes };
}
