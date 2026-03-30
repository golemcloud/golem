const explorer = HostApiExplorer.get("explorer");
const result = await explorer.exploreBlobstore();
console.log(result);
