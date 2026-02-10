import type { Meta, StoryObj } from "@storybook/react-vite";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "./tabs";
import { userEvent, within, expect } from "storybook/test";

const meta = {
  title: "UI/Tabs",
  component: Tabs,
  parameters: {
    skipGlobalRouter: true,
    a11y: {
      config: {
        rules: [{ id: "color-contrast", enabled: false }],
      },
    },
  },
} satisfies Meta<typeof Tabs>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: () => (
    <Tabs defaultValue="overview" className="w-[400px]">
      <TabsList>
        <TabsTrigger value="overview">Overview</TabsTrigger>
        <TabsTrigger value="workers">Workers</TabsTrigger>
        <TabsTrigger value="logs">Logs</TabsTrigger>
      </TabsList>
      <TabsContent value="overview">
        <p className="text-sm text-muted-foreground">
          Component overview and metadata.
        </p>
      </TabsContent>
      <TabsContent value="workers">
        <p className="text-sm text-muted-foreground">
          Active worker instances and their status.
        </p>
      </TabsContent>
      <TabsContent value="logs">
        <p className="text-sm text-muted-foreground">
          Recent execution logs and events.
        </p>
      </TabsContent>
    </Tabs>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Default tab is visible
    await expect(
      canvas.getByText("Component overview and metadata."),
    ).toBeInTheDocument();

    // Switch to Workers tab
    await userEvent.click(canvas.getByText("Workers"));
    await expect(
      canvas.getByText("Active worker instances and their status."),
    ).toBeInTheDocument();

    // Switch to Logs tab
    await userEvent.click(canvas.getByText("Logs"));
    await expect(
      canvas.getByText("Recent execution logs and events."),
    ).toBeInTheDocument();
  },
};

export const TwoTabs: Story = {
  render: () => (
    <Tabs defaultValue="code" className="w-[400px]">
      <TabsList>
        <TabsTrigger value="code">Code</TabsTrigger>
        <TabsTrigger value="preview">Preview</TabsTrigger>
      </TabsList>
      <TabsContent value="code">
        <pre className="rounded-md bg-muted p-4 text-sm">
          {`let result = golem:shopping/api.{add-item}(item);`}
        </pre>
      </TabsContent>
      <TabsContent value="preview">
        <div className="rounded-md border p-4 text-sm">
          Function call preview rendered here.
        </div>
      </TabsContent>
    </Tabs>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Code tab is default
    await expect(canvas.getByText(/add-item/)).toBeInTheDocument();

    // Switch to Preview
    await userEvent.click(canvas.getByText("Preview"));
    await expect(
      canvas.getByText("Function call preview rendered here."),
    ).toBeInTheDocument();
  },
};
