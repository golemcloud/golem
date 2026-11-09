package cloud.golem.annotations

/**
 * Marks an [Endpoint]/agent method as **read-only**: it does not mutate agent state, so Golem may
 * cache its result according to [cache]. Mirrors the Scala SDK's `@readOnly`.
 *
 * [cache] is a DSL string (the same style as `@Agent(snapshotting = ...)`):
 * - `"until-write"` (default) — cache the result until the next state-mutating call;
 * - `"no-cache"` — never cache;
 * - `"ttl(<nanos>)"` — cache for a fixed duration, in nanoseconds (e.g. `"ttl(5000000000)"`).
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class ReadOnly(val cache: String = "until-write")
