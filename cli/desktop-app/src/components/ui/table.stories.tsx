import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TableFooter,
} from "./table";
import { Badge } from "./badge";
import { within, expect } from "storybook/test";

const meta = {
  title: "UI/Table",
  component: Table,
  parameters: {
    skipGlobalRouter: true,
  },
} satisfies Meta<typeof Table>;

export default meta;
type Story = StoryObj<typeof meta>;

const workers = [
  {
    id: "worker-001",
    component: "shopping-cart",
    status: "Running" as const,
    invocations: 1_284,
    lastActive: "2 min ago",
  },
  {
    id: "worker-002",
    component: "shopping-cart",
    status: "Idle" as const,
    invocations: 753,
    lastActive: "15 min ago",
  },
  {
    id: "worker-003",
    component: "order-processor",
    status: "Running" as const,
    invocations: 3_421,
    lastActive: "Just now",
  },
  {
    id: "worker-004",
    component: "inventory",
    status: "Error" as const,
    invocations: 42,
    lastActive: "1 hour ago",
  },
];

const statusVariant = {
  Running: "success",
  Idle: "secondary",
  Error: "destructive",
} as const;

export const Default: Story = {
  render: () => (
    <Table>
      <TableCaption>Active worker instances</TableCaption>
      <TableHeader>
        <TableRow>
          <TableHead>Worker ID</TableHead>
          <TableHead>Component</TableHead>
          <TableHead>Status</TableHead>
          <TableHead className="text-right">Invocations</TableHead>
          <TableHead className="text-right">Last Active</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {workers.map(worker => (
          <TableRow key={worker.id}>
            <TableCell className="font-medium">{worker.id}</TableCell>
            <TableCell>{worker.component}</TableCell>
            <TableCell>
              <Badge variant={statusVariant[worker.status]}>
                {worker.status}
              </Badge>
            </TableCell>
            <TableCell className="text-right">
              {worker.invocations.toLocaleString()}
            </TableCell>
            <TableCell className="text-right">{worker.lastActive}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Verify table headers
    await expect(canvas.getByText("Worker ID")).toBeInTheDocument();
    await expect(canvas.getByText("Component")).toBeInTheDocument();
    await expect(canvas.getByText("Status")).toBeInTheDocument();

    // Verify data rows
    await expect(canvas.getByText("worker-001")).toBeInTheDocument();
    await expect(canvas.getByText("order-processor")).toBeInTheDocument();
    await expect(canvas.getByText("inventory")).toBeInTheDocument();

    // Verify all rows rendered
    const rows = canvas.getAllByRole("row");
    // 1 header row + 4 data rows = 5
    await expect(rows).toHaveLength(5);
  },
};

export const WithFooter: Story = {
  render: () => (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Component</TableHead>
          <TableHead>Version</TableHead>
          <TableHead className="text-right">Size (KB)</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow>
          <TableCell className="font-medium">shopping-cart</TableCell>
          <TableCell>1.0.0</TableCell>
          <TableCell className="text-right">2,400</TableCell>
        </TableRow>
        <TableRow>
          <TableCell className="font-medium">order-processor</TableCell>
          <TableCell>2.1.0</TableCell>
          <TableCell className="text-right">1,850</TableCell>
        </TableRow>
        <TableRow>
          <TableCell className="font-medium">inventory</TableCell>
          <TableCell>1.3.2</TableCell>
          <TableCell className="text-right">3,100</TableCell>
        </TableRow>
      </TableBody>
      <TableFooter>
        <TableRow>
          <TableCell colSpan={2}>Total</TableCell>
          <TableCell className="text-right">7,350</TableCell>
        </TableRow>
      </TableFooter>
    </Table>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Total")).toBeInTheDocument();
    await expect(canvas.getByText("7,350")).toBeInTheDocument();
  },
};

export const Empty: Story = {
  render: () => (
    <Table>
      <TableCaption>No workers found</TableCaption>
      <TableHeader>
        <TableRow>
          <TableHead>Worker ID</TableHead>
          <TableHead>Component</TableHead>
          <TableHead>Status</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow>
          <TableCell colSpan={3} className="h-24 text-center">
            No active workers.
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("No active workers.")).toBeInTheDocument();
  },
};
