The `wasi:keyvalue/cache` interface defines the operations of a single
instance of a "cache", which is a non-durable, weakly-consistent key-value
store. "Non-durable" means that caches are allowed and expected to
arbitrarily discard key-value entries. "Weakly-consistent" means that there
are essentially no guarantees that operations will agree on their results: a
get following a set may not observe the set value; multiple gets may observe
different previous set values; etc. The only guarantee is that values are
not materialized "out of thin air": if a `get` returns a value, that value
was passed to a `set` operation at some point in time in the past.
Additionally, caches MUST make a best effort to respect the supplied
Time-to-Live values (within the usual limitations around time in a
distributed setting).