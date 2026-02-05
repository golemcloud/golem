import type { Meta, StoryObj } from "@storybook/react-vite";
import Navbar from "./navbar";

const meta = {
  title: "Components/Navbar",
  component: Navbar,
} satisfies Meta<typeof Navbar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithNavigation: Story = {
  args: {
    showNav: true,
  },
  parameters: {
    router: {
      route: "/app/my-shopping-app/dashboard",
      path: "/app/:appId/*",
    },
  },
};

export const WithoutNavigation: Story = {
  args: {
    showNav: false,
  },
};

export const NoAppId: Story = {
  args: {
    showNav: true,
  },
};
