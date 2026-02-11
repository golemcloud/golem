import type { Meta, StoryObj } from "@storybook/react-vite";
import { YamlEditor } from "./yaml-editor";
import { fn } from "storybook/test";

const meta = {
  title: "Components/YamlEditor",
  component: YamlEditor,
  args: {
    onChange: fn(),
  },
} satisfies Meta<typeof YamlEditor>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    value: `# Golem Application Configuration
name: my-shopping-app
version: "1.0.0"

components:
  - name: shopping-cart
    type: Durable
    source:
      wit: wit/shopping-cart.wit
      build:
        command: cargo component build --release
        output: target/wasm32-wasip1/release/shopping_cart.wasm

  - name: auth-service
    type: Ephemeral
    source:
      wit: wit/auth-service.wit
      build:
        command: cargo component build --release
        output: target/wasm32-wasip1/release/auth_service.wasm

apis:
  - name: shopping-api
    version: "0.1.0"
    routes:
      - method: Get
        path: /api/products
        component: shopping-cart
        handler: get-products
      - method: Post
        path: /api/cart/add
        component: shopping-cart
        handler: add-item
`,
  },
};

export const Empty: Story = {
  args: {
    value: "",
  },
};

export const WithErrors: Story = {
  args: {
    value: `name: my-app
version: 1.0.0
components:
  - name: broken
    invalid_indentation:
  bad: yaml
    here: too
  - :missing key
`,
  },
};
