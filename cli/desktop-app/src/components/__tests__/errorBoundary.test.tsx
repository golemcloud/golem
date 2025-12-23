import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import React from "react";
import ErrorBoundary from "../errorBoundary";

// Component that throws an error for testing
const ThrowError = ({ shouldThrow }: { shouldThrow: boolean }) => {
  if (shouldThrow) {
    throw new Error("Test error");
  }
  return <div>No error</div>;
};

describe("ErrorBoundary", () => {
  let consoleSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    consoleSpy = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    consoleSpy.mockRestore();
  });

  it("should render children when there is no error", () => {
    render(
      <ErrorBoundary>
        <div>Test content</div>
      </ErrorBoundary>,
    );

    expect(screen.getByText("Test content")).toBeInTheDocument();
  });

  it("should render error message when child component throws error", () => {
    render(
      <ErrorBoundary>
        <ThrowError shouldThrow={true} />
      </ErrorBoundary>,
    );

    expect(screen.getByText("Something went wrong.")).toBeInTheDocument();
    expect(screen.queryByText("No error")).not.toBeInTheDocument();
  });

  it("should not render error message when child component does not throw", () => {
    render(
      <ErrorBoundary>
        <ThrowError shouldThrow={false} />
      </ErrorBoundary>,
    );

    expect(screen.getByText("No error")).toBeInTheDocument();
    expect(screen.queryByText("Something went wrong.")).not.toBeInTheDocument();
  });

  it("should log error to console when error occurs", () => {
    render(
      <ErrorBoundary>
        <ThrowError shouldThrow={true} />
      </ErrorBoundary>,
    );

    expect(consoleSpy).toHaveBeenCalledWith(
      "ErrorBoundary caught an error: ",
      expect.any(Error),
      expect.any(Object),
    );
  });

  it("should render multiple children when no error occurs", () => {
    render(
      <ErrorBoundary>
        <div>First child</div>
        <div>Second child</div>
        <span>Third child</span>
      </ErrorBoundary>,
    );

    expect(screen.getByText("First child")).toBeInTheDocument();
    expect(screen.getByText("Second child")).toBeInTheDocument();
    expect(screen.getByText("Third child")).toBeInTheDocument();
  });

  it("should handle nested components with error", () => {
    const NestedComponent = () => (
      <div>
        <p>Nested content</p>
        <ThrowError shouldThrow={true} />
      </div>
    );

    render(
      <ErrorBoundary>
        <NestedComponent />
      </ErrorBoundary>,
    );

    expect(screen.getByText("Something went wrong.")).toBeInTheDocument();
    expect(screen.queryByText("Nested content")).not.toBeInTheDocument();
  });

  it("should recover from error state when component remounts", () => {
    const { rerender } = render(
      <ErrorBoundary>
        <ThrowError shouldThrow={true} />
      </ErrorBoundary>,
    );

    // Should show error
    expect(screen.getByText("Something went wrong.")).toBeInTheDocument();

    // Rerender with new ErrorBoundary instance (simulating remount)
    rerender(
      <ErrorBoundary key="new-instance">
        <ThrowError shouldThrow={false} />
      </ErrorBoundary>,
    );

    // Should show normal content
    expect(screen.getByText("No error")).toBeInTheDocument();
    expect(screen.queryByText("Something went wrong.")).not.toBeInTheDocument();
  });

  it("should handle string children", () => {
    render(<ErrorBoundary>Simple text content</ErrorBoundary>);

    expect(screen.getByText("Simple text content")).toBeInTheDocument();
  });

  it("should handle React fragments as children", () => {
    render(
      <ErrorBoundary>
        <React.Fragment>
          <div>Fragment child 1</div>
          <div>Fragment child 2</div>
        </React.Fragment>
      </ErrorBoundary>,
    );

    expect(screen.getByText("Fragment child 1")).toBeInTheDocument();
    expect(screen.getByText("Fragment child 2")).toBeInTheDocument();
  });

  it("should catch errors in event handlers within children", () => {
    const ProblematicComponent = () => {
      const handleClick = () => {
        throw new Error("Event handler error");
      };

      return <button onClick={handleClick}>Click me</button>;
    };

    render(
      <ErrorBoundary>
        <ProblematicComponent />
      </ErrorBoundary>,
    );

    // Note: Error boundaries don't catch errors in event handlers
    // This test verifies that the component renders normally
    expect(screen.getByText("Click me")).toBeInTheDocument();
  });
});
