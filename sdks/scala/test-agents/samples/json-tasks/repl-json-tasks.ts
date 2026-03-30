const t = Tasks.get("demo");
const created = await t.createTask({ title: "t1" });
const completed = await t.completeTask(1);
const all = await t.getTasks();
console.log({ created, completed, all });
