import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { ChartContainer, ChartTooltip } from "@/components/ui/chart";
import { Invocation } from "@/types/worker";

interface ProcessedData {
  date: string;
  [key: string]: string | number;
}

const processData = (data: Invocation[]): ProcessedData[] => {
  return data.reduce((acc, curr) => {
    const date = new Date(curr.timestamp);
    const dateKey = date.toLocaleDateString("en-US", {
      month: "short",
      day: "numeric",
    });
    const functionName = curr.function.split(".")[1].replace(/[{}]/g, "");

    const existingDate = acc.find((item) => item.date === dateKey);
    if (existingDate) {
      existingDate[functionName] =
        ((existingDate[functionName] ?? 0) as number) + 1;
    } else {
      acc.push({
        date: dateKey,
        [functionName]: 1,
      });
    }
    return acc;
  }, [] as ProcessedData[]);
};

export function InvocationsChart({ data = [] as Invocation[] }) {
  const chartData = processData(data);

  return (
    <ChartContainer
      config={{
        "initialize-cart": {
          label: "Initialize Cart",
          color: "hsl(var(--success))",
        },
        "add-item": {
          label: "Add Item",
          color: "hsl(var(--primary))",
        },
      }}
      className="h-[400px]"
    >
      <BarChart data={chartData}>
        <CartesianGrid strokeDasharray="3 3" className="stroke-muted" />
        <XAxis dataKey="date" className="text-muted-foreground" />
        <YAxis className="text-muted-foreground" />
        <ChartTooltip
          content={({ active, payload }) => {
            if (active && payload?.length) {
              return (
                <div className="rounded-lg border bg-background p-2 shadow-sm">
                  <div className="grid grid-cols-2 gap-2">
                    <div className="flex flex-col">
                      <span className="text-[0.70rem] uppercase text-muted-foreground">
                        Date
                      </span>
                      <span className="font-bold">
                        {payload[0].payload.date}
                      </span>
                    </div>
                    {payload.map((entry) => (
                      <div key={entry.name} className="flex flex-col">
                        <span className="text-[0.70rem] uppercase text-muted-foreground">
                          {entry.name}
                        </span>
                        <span className="font-bold">{entry.value}</span>
                      </div>
                    ))}
                  </div>
                </div>
              );
            }
            return null;
          }}
        />
        <Bar
          dataKey="initialize-cart"
          fill="var(--success)"
          radius={[4, 4, 0, 0]}
        />
        <Bar dataKey="add-item" fill="var(--primary)" radius={[4, 4, 0, 0]} />
      </BarChart>
    </ChartContainer>
  );
}
