import type { Meta, StoryObj } from "@storybook/react-vite";
import { Badge } from "./badge";
import { within, expect } from "storybook/test";

const meta = {
  title: "UI/Badge",
  component: Badge,
  args: {
    children: "Badge",
  },
  argTypes: {
    variant: {
      control: "select",
      options: [
        "default",
        "secondary",
        "destructive",
        "outline",
        "success",
        "warning",
      ],
    },
  },
  parameters: {
    skipGlobalRouter: true,
  },
} satisfies Meta<typeof Badge>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Badge")).toBeInTheDocument();
  },
};

export const Secondary: Story = {
  args: {
    variant: "secondary",
    children: "Secondary",
  },
};

export const Destructive: Story = {
  args: {
    variant: "destructive",
    children: "Error",
  },
};

export const Outline: Story = {
  args: {
    variant: "outline",
    children: "Outline",
  },
};

export const Success: Story = {
  args: {
    variant: "success",
    children: "Running",
  },
};

export const Warning: Story = {
  args: {
    variant: "warning",
    children: "Pending",
  },
};

export const AllVariants: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      <Badge variant="default">Default</Badge>
      <Badge variant="secondary">Secondary</Badge>
      <Badge variant="destructive">Error</Badge>
      <Badge variant="outline">Outline</Badge>
      <Badge variant="success">Running</Badge>
      <Badge variant="warning">Pending</Badge>
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Default")).toBeInTheDocument();
    await expect(canvas.getByText("Running")).toBeInTheDocument();
    await expect(canvas.getByText("Error")).toBeInTheDocument();
    await expect(canvas.getByText("Pending")).toBeInTheDocument();
  },
};
