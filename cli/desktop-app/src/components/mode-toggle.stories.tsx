import type { Meta, StoryObj } from "@storybook/react-vite";
import { ModeToggle } from "./mode-toggle";
import { userEvent, within, expect, screen } from "storybook/test";

const meta = {
  title: "Components/ModeToggle",
  component: ModeToggle,
} satisfies Meta<typeof ModeToggle>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    const toggleButton = canvas.getByRole("button", { name: /toggle theme/i });
    await userEvent.click(toggleButton);

    // Dropdown renders in a portal, use screen
    const darkOption = await screen.findByText("Dark");
    await expect(darkOption).toBeInTheDocument();
    await expect(screen.getByText("Light")).toBeInTheDocument();
    await expect(screen.getByText("System")).toBeInTheDocument();

    await userEvent.click(darkOption);
  },
};
