const explorer = await HostApiExplorer.get("explore-all");
const result = await explorer.exploreAll();
console.log(result);
