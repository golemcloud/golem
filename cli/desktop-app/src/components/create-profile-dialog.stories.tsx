import type { Meta, StoryObj } from "@storybook/react-vite";
import { CreateProfileDialog } from "./create-profile-dialog";
import { fn, userEvent, within, expect, screen } from "storybook/test";

const meta = {
  title: "Components/CreateProfileDialog",
  component: CreateProfileDialog,
  args: {
    onProfileCreated: fn(),
  },
} satisfies Meta<typeof CreateProfileDialog>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Click the "New Profile" trigger button
    const triggerButton = canvas.getByRole("button", { name: /new profile/i });
    await userEvent.click(triggerButton);

    // Dialog renders in a portal, use screen
    const dialogTitle = await screen.findByText("Create New Profile");
    await expect(dialogTitle).toBeInTheDocument();

    // Fill Profile Name input
    const nameInput = screen.getByLabelText("Profile Name");
    await userEvent.type(nameInput, "test-profile");

    // Fill Component URL input
    const componentUrlInput = screen.getByLabelText("Component Service URL");
    await userEvent.type(componentUrlInput, "http://localhost:9881");

    // Toggle "set as active" checkbox
    const setActiveCheckbox = screen.getByLabelText(
      "Set as active profile after creation",
    );
    await userEvent.click(setActiveCheckbox);

    // Click Cancel to close
    const cancelButton = screen.getByRole("button", { name: /cancel/i });
    await userEvent.click(cancelButton);
  },
};
