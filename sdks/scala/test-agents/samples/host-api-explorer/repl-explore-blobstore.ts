const explorer = await HostApiExplorer.upsert("explorer");
const result = await explorer.exploreBlobstore();
console.log(result);
