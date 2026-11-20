const demo = await DatabaseDemo.upsert("demo");
const types = await demo.typeShowcase();
console.log(types);
