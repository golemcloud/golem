const explorer = await HostApiExplorer.upsert("explorer");
const result = await explorer.exploreOplog();
console.log(result);
