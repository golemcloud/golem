const demo = JsonPromiseDemo.get("json-demo");
const roundtrip = await demo.jsonRoundtrip();
const blocking = await demo.blockingDemo();
console.log({ roundtrip, blocking });
