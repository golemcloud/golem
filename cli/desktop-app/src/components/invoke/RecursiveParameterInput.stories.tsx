import type { Meta, StoryObj } from "@storybook/react-vite";
import { RecursiveParameterInput } from "./RecursiveParameterInput";
import { fn, userEvent, within, expect, screen } from "storybook/test";
import type { Typ } from "@/types/component";

const meta = {
  title: "Components/Invoke/RecursiveParameterInput",
  component: RecursiveParameterInput,
  args: {
    onChange: fn(),
  },
} satisfies Meta<typeof RecursiveParameterInput>;

export default meta;
type Story = StoryObj<typeof meta>;

// --- Primitive Types ---

export const StringInput: Story = {
  args: {
    name: "greeting",
    typeDef: { type: "str" },
    value: "hello world",
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    const input = canvas.getByPlaceholderText("Enter greeting...");
    await userEvent.clear(input);
    await userEvent.type(input, "goodbye world");
    await expect(args.onChange).toHaveBeenCalled();
  },
};

export const NumberU32: Story = {
  args: {
    name: "quantity",
    typeDef: { type: "u32" },
    value: 42,
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    const input = canvas.getByPlaceholderText("Enter quantity...");
    await userEvent.clear(input);
    await userEvent.type(input, "100");
    await expect(args.onChange).toHaveBeenCalled();
  },
};

export const FloatF64: Story = {
  args: {
    name: "price",
    typeDef: { type: "f64" },
    value: 3.14159,
  },
};

export const BooleanInput: Story = {
  args: {
    name: "is-active",
    typeDef: { type: "bool" },
    value: true,
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    const falseRadio = canvas.getByLabelText("False");
    await userEvent.click(falseRadio);
    await expect(args.onChange).toHaveBeenCalled();

    const trueRadio = canvas.getByLabelText("True");
    await userEvent.click(trueRadio);
  },
};

export const CharInput: Story = {
  args: {
    name: "initial",
    typeDef: { type: "chr" },
    value: "A",
  },
};

// --- Record ---

const addressTypeDef: Typ = {
  type: "record",
  fields: [
    { name: "street", typ: { type: "str" } },
    { name: "city", typ: { type: "str" } },
    { name: "state", typ: { type: "str" } },
    { name: "zip", typ: { type: "str" } },
    { name: "country", typ: { type: "str" } },
  ],
};

export const AddressRecord: Story = {
  args: {
    name: "address",
    typeDef: addressTypeDef,
    value: {
      street: "123 Main St",
      city: "Springfield",
      state: "IL",
      zip: "62701",
      country: "US",
    },
  },
};

// --- List ---

export const ListOfStrings: Story = {
  args: {
    name: "fruits",
    typeDef: { type: "list", inner: { type: "str" } },
    value: ["apple", "banana", "cherry"],
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    const addButton = canvas.getByRole("button", { name: /add item/i });
    await userEvent.click(addButton);
    await expect(args.onChange).toHaveBeenCalled();
  },
};

export const ListOfRecords: Story = {
  args: {
    name: "addresses",
    typeDef: { type: "list", inner: addressTypeDef },
    value: [
      {
        street: "123 Main St",
        city: "Springfield",
        state: "IL",
        zip: "62701",
        country: "US",
      },
      {
        street: "456 Oak Ave",
        city: "Portland",
        state: "OR",
        zip: "97201",
        country: "US",
      },
    ],
  },
};

// --- Variant ---

const shapeTypeDef: Typ = {
  type: "variant",
  cases: [
    {
      name: "circle",
      typ: {
        type: "record",
        fields: [{ name: "radius", typ: { type: "f64" } }],
      },
    },
    {
      name: "rectangle",
      typ: {
        type: "record",
        fields: [
          { name: "width", typ: { type: "f64" } },
          { name: "height", typ: { type: "f64" } },
        ],
      },
    },
    {
      name: "triangle",
      typ: {
        type: "record",
        fields: [
          { name: "base", typ: { type: "f64" } },
          { name: "height", typ: { type: "f64" } },
        ],
      },
    },
    { name: "point", typ: { type: "unit" } },
  ],
};

export const ShapeVariant: Story = {
  args: {
    name: "shape",
    typeDef: shapeTypeDef,
    value: { circle: { radius: 5.0 } },
  },
};

// --- Option ---

export const OptionSome: Story = {
  args: {
    name: "nickname",
    typeDef: { type: "option", inner: { type: "str" } },
    value: "present value",
  },
};

export const OptionNone: Story = {
  args: {
    name: "nickname",
    typeDef: { type: "option", inner: { type: "str" } },
    value: null,
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Click checkbox to enable optional value
    const checkbox = canvas.getByRole("checkbox");
    await userEvent.click(checkbox);

    // Controlled component - onChange is called but parent doesn't update value,
    // so we verify the callback was fired
    await expect(args.onChange).toHaveBeenCalled();
  },
};

export const OptionRecord: Story = {
  args: {
    name: "shipping-address",
    typeDef: { type: "option", inner: addressTypeDef },
    value: {
      street: "123 Main St",
      city: "Springfield",
      state: "IL",
      zip: "62701",
      country: "US",
    },
  },
};

// --- Result ---

export const ResultOk: Story = {
  args: {
    name: "response",
    typeDef: { type: "result", ok: { type: "str" }, err: { type: "str" } },
    value: { ok: "success!" },
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Click "Error" radio to switch
    const errorRadio = canvas.getByLabelText("Error");
    await userEvent.click(errorRadio);
    await expect(args.onChange).toHaveBeenCalled();

    // Click "Ok" to switch back
    const okRadio = canvas.getByLabelText("Ok");
    await userEvent.click(okRadio);
  },
};

export const ResultErr: Story = {
  args: {
    name: "response",
    typeDef: { type: "result", ok: { type: "str" }, err: { type: "str" } },
    value: { err: "something went wrong" },
  },
};

export const ResultComplex: Story = {
  args: {
    name: "payment-result",
    typeDef: {
      type: "result",
      ok: {
        type: "record",
        fields: [
          { name: "transaction-id", typ: { type: "str" } },
          { name: "timestamp", typ: { type: "u64" } },
        ],
      },
      err: {
        type: "record",
        fields: [
          { name: "error-code", typ: { type: "u32" } },
          { name: "error-message", typ: { type: "str" } },
        ],
      },
    },
    value: {
      ok: {
        "transaction-id": "txn-abc-123",
        timestamp: 1705312200,
      },
    },
  },
};

// --- Enum ---

export const ColorEnum: Story = {
  args: {
    name: "color",
    typeDef: {
      type: "enum",
      cases: ["red", "green", "blue", "yellow", "cyan", "magenta"],
    },
    value: "green",
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Click the combobox trigger to open the select dropdown
    const combobox = canvas.getByRole("combobox");
    await userEvent.click(combobox);

    // Select renders in portal, use screen
    const blueOption = await screen.findByText("blue");
    await userEvent.click(blueOption);
    await expect(args.onChange).toHaveBeenCalled();
  },
};

// --- Flags ---

export const PermissionsFlags: Story = {
  args: {
    name: "permissions",
    typeDef: {
      type: "flags",
      names: ["read", "write", "execute", "admin", "delete"],
    },
    value: ["read", "write"],
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Toggle "execute" flag on
    const executeCheckbox = canvas.getByLabelText("execute");
    await userEvent.click(executeCheckbox);
    await expect(args.onChange).toHaveBeenCalled();

    // Toggle "read" flag off
    const readCheckbox = canvas.getByLabelText("read");
    await userEvent.click(readCheckbox);
    await expect(args.onChange).toHaveBeenCalledTimes(2);
  },
};

// --- Tuple ---

export const SimpleTuple: Story = {
  args: {
    name: "entry",
    typeDef: {
      type: "tuple",
      fields: [
        { name: "_0", typ: { type: "str" } },
        { name: "_1", typ: { type: "u32" } },
        { name: "_2", typ: { type: "bool" } },
      ],
    },
    value: ["hello", 42, true],
  },
};

// --- Deeply Nested Order ---

const orderTypeDef: Typ = {
  type: "record",
  fields: [
    { name: "order-id", typ: { type: "str" } },
    { name: "customer-name", typ: { type: "str" } },
    { name: "total", typ: { type: "f64" } },
    {
      name: "status",
      typ: {
        type: "enum",
        cases: ["pending", "processing", "shipped", "delivered", "cancelled"],
      },
    },
    {
      name: "shipping-address",
      typ: { type: "option", inner: addressTypeDef },
    },
    {
      name: "items",
      typ: {
        type: "list",
        inner: {
          type: "record",
          fields: [
            { name: "product-name", typ: { type: "str" } },
            { name: "quantity", typ: { type: "u32" } },
            { name: "unit-price", typ: { type: "f64" } },
            {
              name: "discount",
              typ: {
                type: "variant",
                cases: [
                  { name: "none", typ: { type: "unit" } },
                  {
                    name: "percentage",
                    typ: {
                      type: "record",
                      fields: [{ name: "percent", typ: { type: "f64" } }],
                    },
                  },
                  {
                    name: "fixed-amount",
                    typ: {
                      type: "record",
                      fields: [{ name: "amount", typ: { type: "f64" } }],
                    },
                  },
                ],
              },
            },
            {
              name: "tags",
              typ: {
                type: "flags",
                names: ["fragile", "perishable", "oversized", "hazardous"],
              },
            },
          ],
        },
      },
    },
    {
      name: "payment-result",
      typ: {
        type: "result",
        ok: {
          type: "record",
          fields: [
            { name: "transaction-id", typ: { type: "str" } },
            { name: "timestamp", typ: { type: "u64" } },
          ],
        },
        err: {
          type: "record",
          fields: [
            { name: "error-code", typ: { type: "u32" } },
            { name: "error-message", typ: { type: "str" } },
          ],
        },
      },
    },
  ],
};

export const DeeplyNestedOrder: Story = {
  args: {
    name: "order",
    typeDef: orderTypeDef,
    value: {
      "order-id": "ORD-2024-00142",
      "customer-name": "Jane Smith",
      total: 1249.97,
      status: "processing",
      "shipping-address": {
        street: "123 Main St",
        city: "Springfield",
        state: "IL",
        zip: "62701",
        country: "US",
      },
      items: [
        {
          "product-name": "Laptop Pro 16",
          quantity: 1,
          "unit-price": 999.99,
          discount: { percentage: { percent: 10.0 } },
          tags: ["fragile"],
        },
        {
          "product-name": "USB-C Hub",
          quantity: 2,
          "unit-price": 49.99,
          discount: { "fixed-amount": { amount: 5.0 } },
          tags: [],
        },
        {
          "product-name": "Organic Coffee Beans 1kg",
          quantity: 3,
          "unit-price": 24.99,
          discount: "none",
          tags: ["perishable"],
        },
      ],
      "payment-result": {
        ok: {
          "transaction-id": "txn-stripe-abc123def456",
          timestamp: 1705312200,
        },
      },
    },
  },
};

// --- Empty State ---

export const EmptyState: Story = {
  args: {
    name: "message",
    typeDef: { type: "str" },
    value: "",
  },
};
