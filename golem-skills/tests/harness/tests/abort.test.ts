import { describe, it } from "node:test";
import assert from "node:assert/strict";

/**
 * Test the abort signal check in executor step loop.
 * We can't easily test the full executor (needs Golem server),
 * but we verify the AbortController mechanics work as expected.
 */
describe("AbortController integration", () => {
  it("AbortController.abort() sets signal.aborted to true", () => {
    const controller = new AbortController();
    assert.equal(controller.signal.aborted, false);
    controller.abort();
    assert.equal(controller.signal.aborted, true);
  });

  it("optional chaining on undefined abortSignal returns undefined (falsy)", () => {
    const options: { abortSignal?: AbortSignal } = {};
    // This mirrors the check in executor: this.options.abortSignal?.aborted
    assert.equal(options.abortSignal?.aborted, undefined);
    assert.equal(!options.abortSignal?.aborted, true); // falsy, so loop continues
  });

  it("simulates step loop breaking on abort", () => {
    const controller = new AbortController();
    const steps = ["a", "b", "c", "d"];
    const executed: string[] = [];

    // Abort after 2 steps
    for (const step of steps) {
      if (controller.signal.aborted) break;
      executed.push(step);
      if (executed.length === 2) controller.abort();
    }

    assert.deepEqual(executed, ["a", "b"]);
  });

  it("SIGINT handler pattern: first press aborts, second would force exit", () => {
    let interrupted = false;
    const controller = new AbortController();
    const forceExitCalled: boolean[] = [];

    // Simulate the SIGINT handler
    function handleSigint() {
      if (interrupted) {
        forceExitCalled.push(true);
        return;
      }
      interrupted = true;
      controller.abort();
    }

    // First "Ctrl+C"
    handleSigint();
    assert.equal(interrupted, true);
    assert.equal(controller.signal.aborted, true);
    assert.equal(forceExitCalled.length, 0);

    // Second "Ctrl+C"
    handleSigint();
    assert.equal(forceExitCalled.length, 1);
  });
});
