const explorer = await HostApiExplorer.upsert("explorer");
const result = await explorer.exploreContext();
console.log(result);
