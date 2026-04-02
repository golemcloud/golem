const demo = await DatabaseDemo.get("demo");
const types = await demo.typeShowcase();
console.log(types);
