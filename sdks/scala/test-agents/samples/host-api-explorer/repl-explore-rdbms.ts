const explorer = await HostApiExplorer.upsert("explorer");
const result = await explorer.exploreRdbms();
console.log(result);
