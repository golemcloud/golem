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

export type Either<T, E> = { tag: 'right'; val: T } | { tag: 'left'; val: E };

export function right<T, E = never>(val: T): Either<T, E> {
  return { tag: 'right', val };
}

export function left<T = never, E = unknown>(val: E): Either<T, E> {
  return { tag: 'left', val };
}

export function map<T, E, U>(r: Either<T, E>, f: (t: T) => U): Either<U, E> {
  return r.tag === 'right' ? right(f(r.val)) : r;
}

export function getRight<T, E>(r: Either<T, E>): T | null {
  return r.tag === 'right' ? r.val : null;
}

export function flatMap<T, E, U>(
  e: Either<T, E>,
  f: (t: T) => Either<U, E>,
): Either<U, E> {
  return e.tag === 'right' ? f(e.val) : e;
}

export function getOrElse<T, E, U>(
  e: Either<T, E>,
  onErr: (err: E) => U,
): T | U {
  return e.tag === 'right' ? e.val : onErr(e.val);
}

export function getOrThrow<T, E>(e: Either<T, E>): T {
  if (e.tag === 'right') return e.val;
  throw new Error(`Called getOrThrow on an Err value: ${e.val}`);
}

export function getOrThrowWith<T, E>(
  e: Either<T, E>,
  onErr: (err: E) => Error,
): T {
  if (e.tag === 'right') return e.val;
  throw onErr(e.val);
}

export function isLeft<T, E>(r: Either<T, E>): r is { tag: 'left'; val: E } {
  return r.tag === 'left';
}

export function isRight<T, E>(r: Either<T, E>): r is { tag: 'right'; val: T } {
  return r.tag === 'right';
}

export function mapBoth<T, E, U, F>(
  r: Either<T, E>,
  onOk: (t: T) => U,
  onErr: (e: E) => F,
): Either<U, F> {
  return r.tag === 'right' ? right(onOk(r.val)) : left(onErr(r.val));
}

export function zipWith<A, B, C, E>(
  ra: Either<A, E>,
  rb: Either<B, E>,
  f: (a: A, b: B) => C,
): Either<C, E> {
  if (ra.tag === 'left') return ra as Either<C, E>;
  if (rb.tag === 'left') return rb as Either<C, E>;
  return right(f(ra.val, rb.val));
}

export function zipBoth<A, B, E>(
  ra: Either<A, E>,
  rb: Either<B, E>,
): Either<[A, B], E> {
  if (ra.tag === 'left') {
    return { tag: 'left', val: ra.val } as Either<[A, B], E>;
  }
  if (rb.tag === 'left') {
    return { tag: 'left', val: rb.val } as Either<[A, B], E>;
  }
  return right([ra.val, rb.val]);
}

export function all<T, E>(results: Either<T, E>[]): Either<T[], E> {
  const vals: T[] = [];
  for (const r of results) {
    if (r.tag === 'left') return r;
    vals.push(r.val);
  }
  return right(vals);
}
