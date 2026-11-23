const shard = await Shard.upsert("users", 0);
await shard.set("alice", "Alice Smith");
const result = await shard.get("alice");
console.log(result);
