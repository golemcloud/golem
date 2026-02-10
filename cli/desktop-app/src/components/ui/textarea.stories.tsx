import type { Meta, StoryObj } from "@storybook/react-vite";
import { Textarea } from "./textarea";
import { Label } from "./label";
import { userEvent, within, expect } from "storybook/test";

const meta = {
  title: "UI/Textarea",
  component: Textarea,
  parameters: {
    skipGlobalRouter: true,
  },
} satisfies Meta<typeof Textarea>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    placeholder: "Type your message here...",
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const textarea = canvas.getByPlaceholderText("Type your message here...");
    await expect(textarea).toBeInTheDocument();

    await userEvent.type(textarea, "Hello from Storybook");
    await expect(textarea).toHaveValue("Hello from Storybook");
  },
};

export const WithLabel: Story = {
  render: () => (
    <div className="grid w-full gap-1.5">
      <Label htmlFor="message">Your message</Label>
      <Textarea id="message" placeholder="Type your message here..." />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Your message")).toBeInTheDocument();
  },
};

export const Disabled: Story = {
  args: {
    placeholder: "Disabled",
    disabled: true,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      canvas.getByPlaceholderText("Disabled"),
    ).toBeDisabled();
  },
};

export const WithDefaultValue: Story = {
  render: () => (
    <div className="grid w-full gap-1.5">
      <Label htmlFor="rib-input">RIB Expression</Label>
      <Textarea
        id="rib-input"
        defaultValue={"let result = golem:shopping/api.{add-item}(item);\nresult"}
      />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const textarea = canvas.getByRole("textbox");
    await expect(textarea).toHaveValue(
      "let result = golem:shopping/api.{add-item}(item);\nresult",
    );
  },
};
