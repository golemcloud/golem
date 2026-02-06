import type { Meta, StoryObj } from "@storybook/react-vite";
import { LogViewer } from "./log-viewer";
import { fn, userEvent, expect, screen } from "storybook/test";

const meta = {
  title: "Components/LogViewer",
  component: LogViewer,
  args: {
    onOpenChange: fn(),
  },
} satisfies Meta<typeof LogViewer>;

export default meta;
type Story = StoryObj<typeof meta>;

export const SuccessLog: Story = {
  args: {
    isOpen: true,
    title: "Component deployed successfully",
    status: "success",
    operation: "Deploy",
    logs: `[2024-01-15T10:30:00Z] Starting deployment of shopping-cart component...
[2024-01-15T10:30:01Z] Building component from source...
[2024-01-15T10:30:05Z] Compilation successful
[2024-01-15T10:30:06Z] Uploading WASM binary (2.4 MB)...
[2024-01-15T10:30:08Z] Component registered: shopping-cart v1.0.0
[2024-01-15T10:30:09Z] Creating worker instance...
[2024-01-15T10:30:10Z] Worker ready: shopping-cart-worker-001
[2024-01-15T10:30:10Z] Deployment completed successfully`,
  },
  play: async () => {
    // Dialog renders in a portal, use screen
    const title = await screen.findByText("Component deployed successfully");
    await expect(title).toBeInTheDocument();

    // Verify status badge
    await expect(screen.getByText("Deploy")).toBeInTheDocument();

    // Verify log content
    await expect(
      screen.getByText(/Deployment completed successfully/),
    ).toBeInTheDocument();

    // Click Copy button
    const copyButton = screen.getByRole("button", { name: /copy/i });
    await userEvent.click(copyButton);
  },
};

export const ErrorLog: Story = {
  args: {
    isOpen: true,
    title: "Failed to invoke function add-item",
    status: "error",
    operation: "Invoke",
    logs: `[2024-01-15T10:45:00Z] Invoking golem:shopping/api/{add-item}...
[2024-01-15T10:45:01Z] ERROR: Worker execution failed
[2024-01-15T10:45:01Z] Caused by:
  0: Runtime error
  1: Wasm trap: unreachable code reached
  2: Stack trace:
       at add_item (component.wasm:0x1a2b3)
       at handle_request (component.wasm:0x4d5e6)
[2024-01-15T10:45:01Z] Worker state rolled back to last checkpoint`,
  },
  play: async () => {
    // Dialog renders in a portal, use screen
    const title = await screen.findByText(
      "Failed to invoke function add-item",
    );
    await expect(title).toBeInTheDocument();

    // Verify error status elements
    await expect(screen.getByText("Invoke")).toBeInTheDocument();
    await expect(
      screen.getByText(
        "Operation failed. Review the logs above for details.",
      ),
    ).toBeInTheDocument();
  },
};

export const InfoLog: Story = {
  args: {
    isOpen: true,
    title: "Profile configuration details",
    status: "info",
    operation: "Profile Info",
    logs: `Profile: local
Type: Oss
Component URL: http://localhost:9881
Worker URL: http://localhost:9882
Default Format: json
Active: true`,
  },
};

export const LongLog: Story = {
  args: {
    isOpen: true,
    title: "Build output for large component with many dependencies",
    status: "success",
    operation: "Build",
    logs: Array.from(
      { length: 100 },
      (_, i) =>
        `[2024-01-15T10:${String(i).padStart(2, "0")}:00Z] Processing step ${i + 1}/100: ${
          [
            "Compiling dependency",
            "Linking module",
            "Optimizing WASM",
            "Running validations",
            "Generating bindings",
          ][i % 5]
        } (${Math.floor(Math.random() * 500)}ms)`
    ).join("\n"),
  },
};
