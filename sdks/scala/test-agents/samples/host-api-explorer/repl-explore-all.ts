const explorer = await HostApiExplorer.upsert("explore-all");
const result = await explorer.exploreAll();
console.log(result);
