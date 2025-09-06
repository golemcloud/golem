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

export type Either<T, E> = { tag: 'ok'; val: T } | { tag: 'err'; val: E };

export function ok<T, E = never>(val: T): Either<T, E> {
  return { tag: 'ok', val };
}

export function err<T = never, E = unknown>(val: E): Either<T, E> {
  return { tag: 'err', val };
}

export function map<T, E, U>(r: Either<T, E>, f: (t: T) => U): Either<U, E> {
  return r.tag === 'ok' ? ok(f(r.val)) : r;
}

export function mapBoth<T, E, U, F>(
  r: Either<T, E>,
  onOk: (t: T) => U,
  onErr: (e: E) => F,
): Either<U, F> {
  return r.tag === 'ok' ? ok(onOk(r.val)) : err(onErr(r.val));
}

export function zipBoth<A, B, E>(
  ra: Either<A, E>,
  rb: Either<B, E>,
): Either<[A, B], E> {
  if (ra.tag === 'err') {
    return { tag: 'err', val: ra.val } as Either<[A, B], E>;
  }
  if (rb.tag === 'err') {
    return { tag: 'err', val: rb.val } as Either<[A, B], E>;
  }
  return ok([ra.val, rb.val]);
}

export function all<T, E>(results: Either<T, E>[]): Either<T[], E> {
  const vals: T[] = [];
  for (const r of results) {
    if (r.tag === 'err') return r;
    vals.push(r.val);
  }
  return ok(vals);
}
