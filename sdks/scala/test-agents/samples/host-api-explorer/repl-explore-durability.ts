const explorer = HostApiExplorer.get("explorer");
const result = await explorer.exploreDurability();
console.log(result);
