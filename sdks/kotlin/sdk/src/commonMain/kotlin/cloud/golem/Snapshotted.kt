package cloud.golem

/**
 * Opt into snapshot-based updates by mixing this in alongside [BaseAgent] with your state type:
 * `class MyAgent(...) : BaseAgent(), Snapshotted<MyState> { override var state = MyState(...) }`.
 *
 * [state] is auto-serialized by the runtime (KSP derives the codec from `S` at compile time) and
 * wrapped in a principal-carrying envelope, so a manual (snapshot-based) update restores both your
 * state and the caller identity captured at initialize. `S` MUST be a WIT-mappable type (data
 * class, list, map, pair/triple, enum, sealed class, primitive, Datetime, Either); a non-mappable
 * `S` is a compile-time error. An agent that does not mix this in produces an empty snapshot.
 */
interface Snapshotted<S> {
    var state: S
}
