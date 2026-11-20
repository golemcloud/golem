const demo = await TransactionsDemo.upsert("fallible-success-test");
const result = await demo.fallibleSuccessDemo();
console.log(result);
