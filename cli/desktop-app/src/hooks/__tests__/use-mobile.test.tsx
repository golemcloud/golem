import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  afterEach,
  type MockedFunction,
} from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useIsMobile } from "../use-mobile"; // Adjust import path as needed

describe("useIsMobile", () => {
  let mockMatchMedia: MockedFunction<typeof window.matchMedia>;
  let mockAddEventListener: MockedFunction<
    (event: string, callback: Function) => void
  >;
  let mockRemoveEventListener: MockedFunction<
    (event: string, callback: Function) => void
  >;
  let changeCallback: Function | null = null;

  beforeEach(() => {
    // Mock addEventListener and removeEventListener to capture the callback
    mockAddEventListener = vi
      .fn()
      .mockImplementation((event: string, callback: Function) => {
        if (event === "change") {
          changeCallback = callback;
        }
      });

    mockRemoveEventListener = vi
      .fn()
      .mockImplementation((event: string, callback: Function) => {
        if (event === "change" && changeCallback === callback) {
          changeCallback = null;
        }
      });

    // Mock matchMedia
    mockMatchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(), // deprecated
      removeListener: vi.fn(), // deprecated
      addEventListener: mockAddEventListener,
      removeEventListener: mockRemoveEventListener,
      dispatchEvent: vi.fn(),
    }));

    // Set up window mocks
    Object.defineProperty(window, "matchMedia", {
      writable: true,
      value: mockMatchMedia,
    });

    // Mock window.innerWidth
    Object.defineProperty(window, "innerWidth", {
      writable: true,
      configurable: true,
      value: 1024, // Default to desktop width
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
    changeCallback = null;
  });

  it("should initialize as undefined and then set mobile state based on window width", () => {
    // Set mobile width
    Object.defineProperty(window, "innerWidth", {
      writable: true,
      configurable: true,
      value: 500, // Mobile width
    });

    const { result } = renderHook(() => useIsMobile());

    // Should call matchMedia with correct query
    expect(mockMatchMedia).toHaveBeenCalledWith("(max-width: 767px)");

    // Should add event listener
    expect(mockAddEventListener).toHaveBeenCalledWith(
      "change",
      expect.any(Function),
    );

    // Should return true for mobile
    expect(result.current).toBe(true);
  });

  it("should return false for desktop width", () => {
    // Set desktop width
    Object.defineProperty(window, "innerWidth", {
      writable: true,
      configurable: true,
      value: 1024, // Desktop width
    });

    const { result } = renderHook(() => useIsMobile());

    // Should return false for desktop
    expect(result.current).toBe(false);
  });

  it("should return true for tablet width (below 768px)", () => {
    // Set tablet width
    Object.defineProperty(window, "innerWidth", {
      writable: true,
      configurable: true,
      value: 600, // Tablet width
    });

    const { result } = renderHook(() => useIsMobile());

    // Should return true for tablet (considered mobile)
    expect(result.current).toBe(true);
  });

  it("should return false for width exactly at breakpoint", () => {
    // Set width exactly at breakpoint
    Object.defineProperty(window, "innerWidth", {
      writable: true,
      configurable: true,
      value: 768, // Exactly at breakpoint
    });

    const { result } = renderHook(() => useIsMobile());

    // Should return false (768px and above is not mobile)
    expect(result.current).toBe(false);
  });

  it("should update when viewport changes", () => {
    // Start with desktop width
    Object.defineProperty(window, "innerWidth", {
      writable: true,
      configurable: true,
      value: 1024,
    });

    const { result } = renderHook(() => useIsMobile());

    // Should start as desktop
    expect(result.current).toBe(false);

    // Simulate viewport change to mobile
    act(() => {
      Object.defineProperty(window, "innerWidth", {
        writable: true,
        configurable: true,
        value: 500,
      });

      // Trigger the change callback that was registered
      if (changeCallback) {
        changeCallback();
      }
    });

    // Should now be mobile
    expect(result.current).toBe(true);
  });

  it("should update from mobile to desktop", () => {
    // Start with mobile width
    Object.defineProperty(window, "innerWidth", {
      writable: true,
      configurable: true,
      value: 500,
    });

    const { result } = renderHook(() => useIsMobile());

    // Should start as mobile
    expect(result.current).toBe(true);

    // Simulate viewport change to desktop
    act(() => {
      Object.defineProperty(window, "innerWidth", {
        writable: true,
        configurable: true,
        value: 1200,
      });

      // Trigger the change callback
      if (changeCallback) {
        changeCallback();
      }
    });

    // Should now be desktop
    expect(result.current).toBe(false);
  });

  it("should cleanup event listener on unmount", () => {
    const { unmount } = renderHook(() => useIsMobile());

    // Verify event listener was added
    expect(mockAddEventListener).toHaveBeenCalledWith(
      "change",
      expect.any(Function),
    );

    // Unmount the hook
    unmount();

    // Verify event listener was removed
    expect(mockRemoveEventListener).toHaveBeenCalledWith(
      "change",
      expect.any(Function),
    );
  });

  it("should handle multiple viewport changes", () => {
    // Start with desktop
    Object.defineProperty(window, "innerWidth", {
      writable: true,
      configurable: true,
      value: 1024,
    });

    const { result } = renderHook(() => useIsMobile());
    expect(result.current).toBe(false);

    // Change to mobile
    act(() => {
      Object.defineProperty(window, "innerWidth", {
        writable: true,
        configurable: true,
        value: 400,
      });
      if (changeCallback) changeCallback();
    });
    expect(result.current).toBe(true);

    // Change back to desktop
    act(() => {
      Object.defineProperty(window, "innerWidth", {
        writable: true,
        configurable: true,
        value: 900,
      });
      if (changeCallback) changeCallback();
    });
    expect(result.current).toBe(false);

    // Change to tablet (mobile)
    act(() => {
      Object.defineProperty(window, "innerWidth", {
        writable: true,
        configurable: true,
        value: 700,
      });
      if (changeCallback) changeCallback();
    });
    expect(result.current).toBe(true);
  });

  it("should handle edge case where matchMedia is not supported", () => {
    // Remove matchMedia support
    Object.defineProperty(window, "matchMedia", {
      writable: true,
      value: undefined,
    });

    // This would throw in a real scenario, but we can test the hook still works
    // if we modify it to handle this case gracefully
    expect(() => renderHook(() => useIsMobile())).toThrow();
  });

  it("should use correct breakpoint constant", () => {
    renderHook(() => useIsMobile());

    // Verify the exact media query is used (767px = 768 - 1)
    expect(mockMatchMedia).toHaveBeenCalledWith("(max-width: 767px)");
  });
});
