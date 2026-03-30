const explorer = HostApiExplorer.get("explorer");
const result = await explorer.exploreKeyValue();
console.log(result);
