import { PathSegment, PathSegmentNode } from 'golem:agent/common';
import { validateIdentifier } from './identifier';

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

  const nodes: PathSegmentNode[] = [];

  let i = 0;
  while (i < segment.length) {
    if (segment[i] === '{') {
      const end = segment.indexOf('}', i);
      if (end === -1) {
        throw new Error(`Unclosed "{" in path segment "${segment}"`);
      }

      const name = segment.slice(i + 1, end);
      if (!name) {
        throw new Error(`Empty path variable "{}" is not allowed`);
      }

      if (name === 'agent-type' || name === 'agent-version') {
        nodes.push({
          tag: 'system-variable',
          val: name,
        });
      } else {
        validateIdentifier(name);
        nodes.push({
          tag: 'path-variable',
          val: { variableName: name },
        });
      }

      i = end + 1;
      continue;
    }

    let start = i;
    while (i < segment.length && segment[i] !== '{') {
      i++;
    }

    const literal = segment.slice(start, i);
    validateLiteral(literal);

    nodes.push({
      tag: 'literal',
      val: literal,
    });
  }

  return { concat: nodes };
}

function validateLiteral(segment: string) {
  if (segment.includes('{') || segment.includes('}')) {
    throw new Error(`Invalid literal path segment "${segment}"`);
  }
}
