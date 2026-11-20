const demo = await TransactionsDemo.upsert("infallible-test");
const result = await demo.infallibleDemo();
console.log(result);
