const demo = await TransactionsDemo.get("fallible-failure-test");
const result = await demo.fallibleFailureDemo();
console.log(result);
