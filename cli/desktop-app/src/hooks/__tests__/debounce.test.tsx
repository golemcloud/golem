import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useDebounce } from "../debounce";

describe("useDebounce", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("should return initial value immediately", () => {
    const { result } = renderHook(() => useDebounce("initial", 500));

    expect(result.current).toBe("initial");
  });

  it("should debounce value changes with default delay", () => {
    const { result, rerender } = renderHook(
      ({ value }) => useDebounce(value, 500),
      { initialProps: { value: "initial" } },
    );

    expect(result.current).toBe("initial");

    // Change the value
    rerender({ value: "updated" });

    // Value should not have changed yet
    expect(result.current).toBe("initial");

    // Fast forward time by 400ms (less than delay)
    act(() => {
      vi.advanceTimersByTime(400);
    });

    // Value should still not have changed
    expect(result.current).toBe("initial");

    // Fast forward the remaining time to complete the delay
    act(() => {
      vi.advanceTimersByTime(100);
    });

    // Now the value should have changed
    expect(result.current).toBe("updated");
  });

  it("should use custom delay", () => {
    const { result, rerender } = renderHook(
      ({ value }) => useDebounce(value, 1000),
      { initialProps: { value: "initial" } },
    );

    rerender({ value: "updated" });

    // Fast forward by 500ms (less than custom delay)
    act(() => {
      vi.advanceTimersByTime(500);
    });

    expect(result.current).toBe("initial");

    // Fast forward by remaining 500ms to complete 1000ms delay
    act(() => {
      vi.advanceTimersByTime(500);
    });

    expect(result.current).toBe("updated");
  });

  it("should reset timer on rapid value changes", () => {
    const { result, rerender } = renderHook(
      ({ value }) => useDebounce(value, 500),
      { initialProps: { value: "initial" } },
    );

    // First change
    rerender({ value: "first" });

    // Wait 300ms
    act(() => {
      vi.advanceTimersByTime(300);
    });

    expect(result.current).toBe("initial");

    // Second change before first completes
    rerender({ value: "second" });

    // Wait another 300ms (600ms total, but timer was reset)
    act(() => {
      vi.advanceTimersByTime(300);
    });

    expect(result.current).toBe("initial");

    // Wait remaining 200ms to complete the second change's delay
    act(() => {
      vi.advanceTimersByTime(200);
    });

    expect(result.current).toBe("second");
  });

  it("should handle multiple rapid changes and only use the final value", () => {
    const { result, rerender } = renderHook(
      ({ value }) => useDebounce(value, 300),
      { initialProps: { value: "initial" } },
    );

    // Multiple rapid changes
    rerender({ value: "change1" });

    act(() => {
      vi.advanceTimersByTime(100);
    });

    rerender({ value: "change2" });

    act(() => {
      vi.advanceTimersByTime(100);
    });

    rerender({ value: "final" });

    // Value should still be initial
    expect(result.current).toBe("initial");

    // Complete the delay
    act(() => {
      vi.advanceTimersByTime(300);
    });

    // Should jump directly to final value, skipping intermediate changes
    expect(result.current).toBe("final");
  });

  it("should work with different data types", () => {
    // Test with numbers
    const { result: numberResult, rerender: numberRerender } = renderHook(
      ({ value }) => useDebounce(value, 200),
      { initialProps: { value: 0 } },
    );

    numberRerender({ value: 42 });

    act(() => {
      vi.advanceTimersByTime(200);
    });

    expect(numberResult.current).toBe(42);

    // Test with objects
    const { result: objectResult, rerender: objectRerender } = renderHook(
      ({ value }) => useDebounce(value, 200),
      { initialProps: { value: { name: "initial" } } },
    );

    const newObject = { name: "updated" };
    objectRerender({ value: newObject });

    act(() => {
      vi.advanceTimersByTime(200);
    });

    expect(objectResult.current).toBe(newObject);

    // Test with arrays
    const { result: arrayResult, rerender: arrayRerender } = renderHook(
      ({ value }) => useDebounce(value, 200),
      { initialProps: { value: ["initial"] } },
    );

    const newArray = ["updated", "values"];
    arrayRerender({ value: newArray });

    act(() => {
      vi.advanceTimersByTime(200);
    });

    expect(arrayResult.current).toBe(newArray);
  });

  it("should handle delay changes", () => {
    const { result, rerender } = renderHook(
      ({ value, delay }) => useDebounce(value, delay),
      { initialProps: { value: "initial", delay: 500 } },
    );

    // Change value and delay simultaneously
    rerender({ value: "updated", delay: 1000 });

    // Wait for original delay (500ms)
    act(() => {
      vi.advanceTimersByTime(500);
    });

    // Should not have updated yet due to new delay
    expect(result.current).toBe("initial");

    // Wait for new delay to complete (500ms more)
    act(() => {
      vi.advanceTimersByTime(500);
    });

    expect(result.current).toBe("updated");
  });

  it("should clean up timers when unmounted", () => {
    const clearTimeoutSpy = vi.spyOn(global, "clearTimeout");

    const { unmount } = renderHook(() => useDebounce("test", 500));

    unmount();

    expect(clearTimeoutSpy).toHaveBeenCalled();

    clearTimeoutSpy.mockRestore();
  });

  it("should handle zero delay", () => {
    const { result, rerender } = renderHook(
      ({ value }) => useDebounce(value, 0),
      { initialProps: { value: "initial" } },
    );

    rerender({ value: "updated" });

    // Even with 0 delay, should wait for next tick
    expect(result.current).toBe("initial");

    act(() => {
      vi.advanceTimersByTime(0);
    });

    expect(result.current).toBe("updated");
  });

  it("should use default delay when not provided", () => {
    const { result, rerender } = renderHook(
      ({ value }) => useDebounce(value), // No delay specified, should use 500ms default
      { initialProps: { value: "initial" } },
    );

    rerender({ value: "updated" });

    act(() => {
      vi.advanceTimersByTime(400);
    });

    expect(result.current).toBe("initial");

    act(() => {
      vi.advanceTimersByTime(100);
    });

    expect(result.current).toBe("updated");
  });
});
