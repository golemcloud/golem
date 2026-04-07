const explorer = await HostApiExplorer.get("explorer2");
const result = await explorer.exploreConfig();
console.log(result);
