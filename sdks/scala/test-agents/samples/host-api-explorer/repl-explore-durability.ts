const explorer = await HostApiExplorer.get("explorer");
const result = await explorer.exploreDurability();
console.log(result);
