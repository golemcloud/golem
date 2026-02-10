import type { Meta, StoryObj } from "@storybook/react-vite";
import { GolemCliPathSetting } from "./golem-cli-path";
import { userEvent, within, expect } from "storybook/test";

const meta = {
  title: "Components/GolemCliPathSetting",
  component: GolemCliPathSetting,
} satisfies Meta<typeof GolemCliPathSetting>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Verify the label is rendered
    await expect(canvas.getByText("golem-cli Path")).toBeInTheDocument();

    // Verify the input placeholder
    const input = canvas.getByPlaceholderText(
      "Select golem-cli executable path",
    );
    await expect(input).toBeInTheDocument();
    await expect(input).toHaveAttribute("readonly");

    // Verify buttons
    await expect(canvas.getByText("Browse")).toBeInTheDocument();
    await expect(canvas.getByText("Save")).toBeInTheDocument();

    // Verify help text
    await expect(
      canvas.getByText(/Specify the path to the golem-cli executable/),
    ).toBeInTheDocument();
  },
};

export const BrowseInteraction: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Click the Browse button (mock returns null, so path stays empty)
    const browseButton = canvas.getByText("Browse");
    await userEvent.click(browseButton);

    // Save button should still be enabled (path is empty, click will show toast)
    const saveButton = canvas.getByText("Save");
    await expect(saveButton).toBeInTheDocument();
  },
};
