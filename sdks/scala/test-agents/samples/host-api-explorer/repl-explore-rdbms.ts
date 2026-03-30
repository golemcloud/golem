const explorer = HostApiExplorer.get("explorer");
const result = await explorer.exploreRdbms();
console.log(result);
