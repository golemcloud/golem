const explorer = await HostApiExplorer.get("explorer");
const result = await explorer.exploreOplog();
console.log(result);
