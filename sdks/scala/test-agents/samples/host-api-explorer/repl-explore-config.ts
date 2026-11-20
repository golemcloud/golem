const explorer = await HostApiExplorer.upsert("explorer2");
const result = await explorer.exploreConfig();
console.log(result);
