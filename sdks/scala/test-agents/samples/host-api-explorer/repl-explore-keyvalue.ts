const explorer = await HostApiExplorer.upsert("explorer");
const result = await explorer.exploreKeyValue();
console.log(result);
