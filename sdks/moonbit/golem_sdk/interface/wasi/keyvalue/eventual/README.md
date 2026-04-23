A keyvalue interface that provides eventually consistent CRUD operations.

A CRUD operation is an operation that acts on a single key-value pair.

The value in the key-value pair is defined as a `u8` byte array and the intention
is that it is the common denominator for all data types defined by different
key-value stores to handle data, ensuring compatibility between different
key-value stores. Note: the clients will be expecting serialization/deserialization overhead
to be handled by the key-value store. The value could be a serialized object from
JSON, HTML or vendor-specific data types like AWS S3 objects.

Data consistency in a key value store refers to the gaurantee that once a
write operation completes, all subsequent read operations will return the
value that was written.

The level of consistency in readwrite interfaces is **eventual consistency**,
which means that if a write operation completes successfully, all subsequent
read operations will eventually return the value that was written. In other words,
if we pause the updates to the system, the system eventually will return
the last updated value for read.