import type { Meta, StoryObj } from "@storybook/react-vite";
import { RibEditor } from "./rib-editor";
import { fn } from "storybook/test";

const meta = {
  title: "Components/RibEditor",
  component: RibEditor,
  args: {
    onChange: fn(),
  },
} satisfies Meta<typeof RibEditor>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    value: `let request = golem:shopping/api/{add-item}("Laptop Pro 16", 1, 999.99);
let cart = golem:shopping/api/{get-cart}();
let total = reduce cart from 0.0 (acc, item) => acc + (item.unit-price * item.quantity);
if total > 100.0 then
  golem:shopping/api/{checkout}("credit-card")
else
  "Cart total too low for checkout"`,
  },
};

export const WithScriptKeys: Story = {
  args: {
    value: `golem:shopping/api/{add-item}("Laptop", 1, 999.99)`,
    scriptKeys: [
      "golem:shopping/api/{add-item}",
      "golem:shopping/api/{get-cart}",
      "golem:shopping/api/{checkout}",
      "golem:shopping/api/{remove-item}",
      "golem:auth/api/{login}",
      "golem:auth/api/{logout}",
    ],
  },
};

export const Disabled: Story = {
  args: {
    value: `// This editor is read-only
let result = golem:shopping/api/{get-cart}();
result`,
    disabled: true,
  },
};

export const WithSuggestions: Story = {
  args: {
    value: "",
    scriptKeys: [
      "golem:shopping/api/{add-item}",
      "golem:shopping/api/{get-cart}",
    ],
    suggestVariable: {
      request: {
        method: "GET",
        path: "/api/products",
        headers: {},
      },
      response: {
        status: 200,
        body: "[]",
      },
    },
  },
};
