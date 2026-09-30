import { checkFixture } from "../test-capability-exports/check.mjs";

await checkFixture(process.argv[2], "middleware");
