import type { Meta, StoryObj } from "@storybook/react-vite";
import ErrorBoundary from "./errorBoundary";

const ThrowingChild = () => {
  throw new Error("Test error for storybook");
};

const meta = {
  title: "Components/ErrorBoundary",
  component: ErrorBoundary,
} satisfies Meta<typeof ErrorBoundary>;

export default meta;
type Story = StoryObj<typeof meta>;

export const NoError: Story = {
  args: {
    children: (
      <div className="p-4">
        <h2 className="text-lg font-bold">Everything is fine</h2>
        <p>This child renders without errors.</p>
      </div>
    ),
  },
};

export const WithError: Story = {
  args: {
    children: <ThrowingChild />,
  },
};
