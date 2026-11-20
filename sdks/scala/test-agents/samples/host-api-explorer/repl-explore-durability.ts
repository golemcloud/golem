const explorer = await HostApiExplorer.upsert("explorer");
const result = await explorer.exploreDurability();
console.log(result);
