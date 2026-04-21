A keyvalue interface that provides atomic operations.

Atomic operations are single, indivisible operations. When a fault causes
an atomic operation to fail, it will appear to the invoker of the atomic
operation that the action either completed successfully or did nothing
at all.