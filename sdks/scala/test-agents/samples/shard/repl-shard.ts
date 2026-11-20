const shard = await Shard.upsert("users", 0);
await shard.set("alice", "Alice Smith");
const result = await shard.upsert("alice");
console.log(result);
