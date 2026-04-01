const explorer = await HostApiExplorer.get("explorer");
const result = await explorer.exploreBlobstore();
console.log(result);
