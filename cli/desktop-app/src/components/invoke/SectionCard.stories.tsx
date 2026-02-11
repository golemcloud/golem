import type { Meta, StoryObj } from "@storybook/react-vite";
import { SectionCard } from "./SectionCard";
import { fn, userEvent, within, expect } from "storybook/test";
import type { ComponentExportFunction } from "@/types/component";

const sampleFunctionDetails: ComponentExportFunction = {
  name: "add-item",
  parameters: [
    { name: "product-name", type: "str", typ: { type: "str" } },
    { name: "quantity", type: "u32", typ: { type: "u32" } },
    { name: "unit-price", type: "f64", typ: { type: "f64" } },
  ],
  results: [
    {
      name: null,
      typ: {
        type: "result",
        ok: { type: "str" },
        err: { type: "str" },
      },
    },
  ],
};

const meta = {
  title: "Components/Invoke/SectionCard",
  component: SectionCard,
  args: {
    onInvoke: fn(),
    onReset: fn(),
    copyToClipboard: fn(),
    onValueChange: fn(),
  },
} satisfies Meta<typeof SectionCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Editable: Story = {
  args: {
    title: "Preview",
    description: "Preview the current function invocation arguments",
    value: JSON.stringify(
      {
        params: [
          { name: "product-name", value: "Laptop Pro 16" },
          { name: "quantity", value: 1 },
          { name: "unit-price", value: 999.99 },
        ],
      },
      null,
      2,
    ),
    readOnly: false,
    functionDetails: sampleFunctionDetails,
    exportName: "golem:shopping/api",
    functionName: "add-item",
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Clear and type in the textarea
    const textarea = canvas.getByPlaceholderText("Enter JSON data...");
    await userEvent.clear(textarea);
    await userEvent.type(textarea, '{{"test": true}}');
    await expect(args.onValueChange).toHaveBeenCalled();

    // Click Invoke button
    const invokeButton = canvas.getByRole("button", { name: /invoke/i });
    await userEvent.click(invokeButton);
    await expect(args.onInvoke).toHaveBeenCalled();
  },
};

export const ReadOnly: Story = {
  args: {
    title: "Result",
    description: "View the result of your latest invocation",
    value: JSON.stringify(
      { ok: "Item added successfully: Laptop Pro 16 x1" },
      null,
      2,
    ),
    readOnly: true,
    functionDetails: sampleFunctionDetails,
    exportName: "golem:shopping/api",
    functionName: "add-item",
  },
};

export const WithFunctionDetails: Story = {
  args: {
    title: "Types",
    description: "Types of the function arguments",
    value: JSON.stringify(
      {
        parameters: [
          { name: "product-name", type: "string" },
          { name: "quantity", type: "u32" },
          { name: "unit-price", type: "f64" },
        ],
        results: [{ type: "result<string, string>" }],
      },
      null,
      2,
    ),
    readOnly: true,
    functionDetails: sampleFunctionDetails,
    exportName: "golem:shopping/api",
    functionName: "add-item",
  },
};

export const HttpHandlerWarning: Story = {
  args: {
    title: "Preview",
    description: "Preview the HTTP handler invocation",
    value: JSON.stringify({ method: "GET", path: "/api/health" }, null, 2),
    readOnly: false,
    functionDetails: {
      name: "handle",
      parameters: [
        {
          name: "request",
          type: "record",
          typ: {
            type: "record",
            fields: [
              { name: "method", typ: { type: "str" } },
              { name: "path", typ: { type: "str" } },
            ],
          },
        },
      ],
      results: [],
    },
    exportName: "wasi:http/incoming-handler",
    functionName: "handle",
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Verify warning text is visible
    await expect(
      canvas.getByText("Cannot invoke HTTP handler directly"),
    ).toBeInTheDocument();
  },
};

export const WithCopyButton: Story = {
  args: {
    title: "Preview",
    description: "Click copy to copy JSON to clipboard",
    value: JSON.stringify({ greeting: "Hello, Golem!" }, null, 2),
    readOnly: false,
    functionDetails: sampleFunctionDetails,
    exportName: "golem:shopping/api",
    functionName: "add-item",
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Click Copy button
    const copyButton = canvas.getByRole("button", { name: /copy/i });
    await userEvent.click(copyButton);
    await expect(args.copyToClipboard).toHaveBeenCalled();

    // Click Reset button
    const resetButton = canvas.getByRole("button", { name: /reset/i });
    await userEvent.click(resetButton);
    await expect(args.onReset).toHaveBeenCalled();
  },
};
