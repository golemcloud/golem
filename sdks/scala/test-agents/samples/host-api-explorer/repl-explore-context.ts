const explorer = await HostApiExplorer.get("explorer");
const result = await explorer.exploreContext();
console.log(result);
