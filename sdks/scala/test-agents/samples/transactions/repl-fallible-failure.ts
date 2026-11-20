const demo = await TransactionsDemo.upsert("fallible-failure-test");
const result = await demo.fallibleFailureDemo();
console.log(result);
