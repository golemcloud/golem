package cloud.golem.runtime.config

// Native port of Scala's golem.config.Config[T] (sdks/scala/model/src/main/scala/golem/config/
// Config.scala): a tiny lazy-loaded config value wrapper, package-private in Scala
// (`private[golem]`) -- infrastructure for future annotation-driven config plumbing (Task
// the extended annotations), not directly user-facing. Kept equally minimal here.

/** A lazily-loaded configuration value. */
class Config<T> internal constructor(private val loadFn: () -> T) {
    val value: T get() = loadFn()

    companion object {
        internal fun <T> of(loadFn: () -> T): Config<T> = Config(loadFn)
        internal fun <T> eager(value: T): Config<T> = Config { value }
    }
}
