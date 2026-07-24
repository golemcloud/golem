A keyvalue interface that provides eventually consistent batch operations.

A batch operation is an operation that operates on multiple keys at once.

Batch operations are useful for reducing network round-trip time. For example,
if you want to get the values associated with 100 keys, you can either do 100 get
operations or you can do 1 batch get operation. The batch operation is
faster because it only needs to make 1 network call instead of 100.

A batch operation does not guarantee atomicity, meaning that if the batch
operation fails, some of the keys may have been modified and some may not.
Transactional operations are being worked on and will be added in the future to
provide atomicity.

Data consistency in a key value store refers to the gaurantee that once a
write operation completes, all subsequent read operations will return the
value that was written.

The level of consistency in batch operations is **eventual consistency**, the same
with the readwrite interface. This interface does not guarantee strong consistency,
meaning that if a write operation completes, subsequent read operations may not return
the value that was written.