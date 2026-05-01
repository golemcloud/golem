/**
 * @since 0.1.0
 */
import { Pipeable } from "effect"

/**
 * Make a plain object pipeable by setting its prototype to
 * `Pipeable.Prototype`. Returns a fresh object with the same own
 * enumerable properties as `obj`; `pipe` is a prototype-delegated
 * (i.e. non-own) method, so it does NOT show up in:
 *
 * - `Object.keys(...)`
 * - object spread (`{ ...x }`)
 * - `JSON.stringify(...)`
 *
 * It does still appear in `for...in` traversals because `Pipeable.Prototype.pipe`
 * is enumerable on the prototype — that mirrors how every other
 * pipeable Effect value (`Effect`, `Schema`, `Layer`, ...) behaves.
 *
 * Used on factory return values for declarative DSL records
 * (`EndpointDef`, `MountDef`, `MethodSpec`) so users can compose
 * cross-cutting facets with the canonical Effect `.pipe(...)` style.
 * The literal-object form (`{ ..., http: [...] }`) keeps working — the
 * pipeable form is purely additive.
 *
 * **Example**
 *
 * ```ts
 * Http.get("/x").pipe(Http.withAuth(true), Http.withCors("https://x.com"))
 * method({ params: ..., success: ... }).pipe(
 *   Method.withHttp(Http.get("/x")),
 *   Method.withDescription("..."),
 * )
 * ```
 *
 * @since 0.1.0
 * @category utils
 */
export const withPipe = <T extends object>(obj: T): T & Pipeable.Pipeable =>
  Object.assign(Object.create(Pipeable.Prototype) as object, obj) as T & Pipeable.Pipeable
