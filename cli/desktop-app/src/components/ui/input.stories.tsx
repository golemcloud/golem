import type { Meta, StoryObj } from "@storybook/react-vite";
import { Input } from "./input";
import { Label } from "./label";
import { userEvent, within, expect } from "storybook/test";

const meta = {
  title: "UI/Input",
  component: Input,
  parameters: {
    skipGlobalRouter: true,
  },
} satisfies Meta<typeof Input>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    placeholder: "Enter text...",
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByPlaceholderText("Enter text...");
    await expect(input).toBeInTheDocument();

    await userEvent.type(input, "Hello World");
    await expect(input).toHaveValue("Hello World");
  },
};

export const WithLabel: Story = {
  render: () => (
    <div className="grid w-full max-w-sm items-center gap-1.5">
      <Label htmlFor="email-input">Email</Label>
      <Input type="email" id="email-input" placeholder="Email" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Email")).toBeInTheDocument();
    await expect(canvas.getByPlaceholderText("Email")).toBeInTheDocument();
  },
};

export const Disabled: Story = {
  args: {
    placeholder: "Disabled input",
    disabled: true,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByPlaceholderText("Disabled input");
    await expect(input).toBeDisabled();
  },
};

export const ReadOnly: Story = {
  render: () => (
    <div className="grid w-full max-w-sm items-center gap-1.5">
      <Label htmlFor="readonly-input">CLI Path</Label>
      <Input id="readonly-input" value="/usr/local/bin/golem-cli" readOnly />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByDisplayValue("/usr/local/bin/golem-cli");
    await expect(input).toHaveAttribute("readonly");
  },
};

export const Password: Story = {
  args: {
    type: "password",
    placeholder: "Enter password",
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByPlaceholderText("Enter password");
    await expect(input).toHaveAttribute("type", "password");
  },
};

export const File: Story = {
  render: () => (
    <div className="grid w-full max-w-sm items-center gap-1.5">
      <Label htmlFor="file-input">Upload file</Label>
      <Input id="file-input" type="file" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByLabelText("Upload file");
    await expect(input).toHaveAttribute("type", "file");
  },
};
