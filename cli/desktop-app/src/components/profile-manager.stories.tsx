import type { Meta, StoryObj } from "@storybook/react-vite";
import { ProfileManager } from "./profile-manager";
import { expect, screen } from "storybook/test";

const meta = {
  title: "Components/ProfileManager",
  component: ProfileManager,
} satisfies Meta<typeof ProfileManager>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async () => {
    // Wait for the heading "CLI Profiles" to appear (after loading)
    const heading = await screen.findByText("CLI Profiles");
    await expect(heading).toBeInTheDocument();

    // Verify "New Profile" and "Refresh" buttons exist
    await expect(
      screen.getByRole("button", { name: /new profile/i }),
    ).toBeInTheDocument();
    await expect(
      screen.getByRole("button", { name: /refresh/i }),
    ).toBeInTheDocument();
  },
};
