const shard = await Shard.get("users", 0);
await shard.set("alice", "Alice Smith");
const result = await shard.get("alice");
console.log(result);
