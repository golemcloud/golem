import type { Meta, StoryObj } from "@storybook/react-vite";
import { InvokeLayout } from "./InvokeLayout";
import { fn, userEvent, within, expect } from "storybook/test";
import type { Export, ComponentExportFunction } from "@/types/component";

const addItemFn: ComponentExportFunction = {
  name: "add-item",
  parameters: [
    { name: "product-name", type: "str", typ: { type: "str" } },
    { name: "quantity", type: "u32", typ: { type: "u32" } },
    { name: "unit-price", type: "f64", typ: { type: "f64" } },
  ],
  results: [
    {
      name: null,
      typ: { type: "result", ok: { type: "str" }, err: { type: "str" } },
    },
  ],
};

const getCartFn: ComponentExportFunction = {
  name: "get-cart",
  parameters: [],
  results: [
    {
      name: null,
      typ: {
        type: "list",
        inner: {
          type: "record",
          fields: [
            { name: "product-name", typ: { type: "str" } },
            { name: "quantity", typ: { type: "u32" } },
            { name: "unit-price", typ: { type: "f64" } },
          ],
        },
      },
    },
  ],
};

const checkoutFn: ComponentExportFunction = {
  name: "checkout",
  parameters: [
    {
      name: "payment-method",
      type: "str",
      typ: { type: "str" },
    },
  ],
  results: [
    {
      name: null,
      typ: {
        type: "result",
        ok: {
          type: "record",
          fields: [
            { name: "order-id", typ: { type: "str" } },
            { name: "total", typ: { type: "f64" } },
          ],
        },
        err: { type: "str" },
      },
    },
  ],
};

const shoppingExports: Export[] = [
  {
    name: "golem:shopping/api",
    type: "function",
    functions: [addItemFn, getCartFn, checkoutFn],
  },
];

const meta = {
  title: "Components/Invoke/InvokeLayout",
  component: InvokeLayout,
  args: {
    onNavigateToFunction: fn(),
    onValueChange: fn(),
    onInvoke: fn(),
    copyToClipboard: fn(),
    setViewMode: fn(),
    setValue: fn(),
    setResultValue: fn(),
  },
  parameters: {
    router: {
      route: "/app/my-app/components/shopping-cart/invoke/add-item",
      path: "/app/:appId/components/:componentId/invoke/:fn",
    },
  },
} satisfies Meta<typeof InvokeLayout>;

export default meta;
type Story = StoryObj<typeof meta>;

export const FormMode: Story = {
  args: {
    parsedExports: shoppingExports,
    name: "golem:shopping/api",
    urlFn: "add-item",
    functionDetails: addItemFn,
    viewMode: "form",
    value: "",
    resultValue: "",
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Verify sidebar lists all functions
    await expect(canvas.getByText("add-item")).toBeInTheDocument();
    await expect(canvas.getByText("get-cart")).toBeInTheDocument();
    await expect(canvas.getByText("checkout")).toBeInTheDocument();

    // Click "get-cart" -> assert onNavigateToFunction
    await userEvent.click(canvas.getByText("get-cart"));
    await expect(args.onNavigateToFunction).toHaveBeenCalledWith(
      "golem:shopping/api",
      "get-cart",
    );

    // Click "Json Layout" -> assert setViewMode("preview")
    await userEvent.click(canvas.getByText("Json Layout"));
    await expect(args.setViewMode).toHaveBeenCalledWith("preview");

    // Click "Types" -> assert setViewMode("types")
    await userEvent.click(canvas.getByText("Types"));
    await expect(args.setViewMode).toHaveBeenCalledWith("types");
  },
};

export const PreviewMode: Story = {
  args: {
    parsedExports: shoppingExports,
    name: "golem:shopping/api",
    urlFn: "add-item",
    functionDetails: addItemFn,
    viewMode: "preview",
    value: JSON.stringify(
      {
        params: [
          { name: "product-name", value: "Laptop Pro 16" },
          { name: "quantity", value: 1 },
          { name: "unit-price", value: 999.99 },
        ],
      },
      null,
      2
    ),
    resultValue: "",
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Verify header text
    await expect(
      canvas.getByText("golem:shopping/api - add-item"),
    ).toBeInTheDocument();

    // Click "Form Layout" -> assert setViewMode("form")
    await userEvent.click(canvas.getByText("Form Layout"));
    await expect(args.setViewMode).toHaveBeenCalledWith("form");
  },
};

export const TypesMode: Story = {
  args: {
    parsedExports: shoppingExports,
    name: "golem:shopping/api",
    urlFn: "add-item",
    functionDetails: addItemFn,
    viewMode: "types",
    value: "",
    resultValue: "",
  },
};

export const WithResult: Story = {
  args: {
    parsedExports: shoppingExports,
    name: "golem:shopping/api",
    urlFn: "checkout",
    functionDetails: checkoutFn,
    viewMode: "preview",
    value: JSON.stringify(
      { params: [{ name: "payment-method", value: "credit-card" }] },
      null,
      2
    ),
    resultValue: JSON.stringify(
      { ok: { "order-id": "ORD-2024-00142", total: 1249.97 } },
      null,
      2
    ),
  },
};
