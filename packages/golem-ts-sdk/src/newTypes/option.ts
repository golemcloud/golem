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

export type Option<T> = { tag: 'some'; val: T } | { tag: 'none' };

// Constructors
export function some<T>(val: T): Option<T> {
  return { tag: 'some', val };
}

export function none<T = never>(): Option<T> {
  return { tag: 'none' };
}

export function fromNullable<T>(val: T | null | undefined): Option<T> {
  return val === null || val === undefined ? none() : some(val);
}

export function isSome<T>(opt: Option<T>): opt is { tag: 'some'; val: T } {
  return opt.tag === 'some';
}

export function isNone<T>(opt: Option<T>): opt is { tag: 'none' } {
  return opt.tag === 'none';
}

export function getOrElse<T, U>(opt: Option<T>, onNone: () => U): T | U {
  return opt.tag === 'some' ? opt.val : onNone();
}

export function getOrThrowWith<T>(opt: Option<T>, onNone: () => Error): T {
  if (opt.tag === 'some') return opt.val;
  throw onNone();
}

export function map<T, U>(opt: Option<T>, f: (t: T) => U): Option<U> {
  return opt.tag === 'some' ? some(f(opt.val)) : none();
}

export function mapOr<T, U>(opt: Option<T>, def: U, f: (t: T) => U): U {
  return opt.tag === 'some' ? f(opt.val) : def;
}

export function andThen<T, U>(
  opt: Option<T>,
  f: (t: T) => Option<U>,
): Option<U> {
  return opt.tag === 'some' ? f(opt.val) : none();
}

export function zipWith<A, B, C>(
  oa: Option<A>,
  ob: Option<B>,
  f: (a: A, b: B) => C,
): Option<C> {
  return oa.tag === 'some' && ob.tag === 'some'
    ? some(f(oa.val, ob.val))
    : none();
}

export function all<T>(opts: Option<T>[]): Option<T[]> {
  const vals: T[] = [];
  for (const opt of opts) {
    if (opt.tag === 'none') return none();
    vals.push(opt.val);
  }
  return some(vals);
}
