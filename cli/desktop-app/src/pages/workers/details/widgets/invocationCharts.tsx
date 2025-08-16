import { ChartContainer, ChartTooltip } from "@/components/ui/chart";
import { Invocation } from "@/types/worker";
import { Bar, BarChart, CartesianGrid, XAxis, YAxis } from "recharts";

interface ProcessedData {
  date: string;
  [key: string]: string | number;
}

const processData = (data: Invocation[]): ProcessedData[] => {
  const groupedData: Record<string, ProcessedData> = {};
  const functionSet = new Set<string>();

  // Group data by 4 hour intervals and collect unique function names
  data.forEach(curr => {
    const date = new Date(curr.timestamp);
    date.setMinutes(0, 0, 0);
    const hour = date.getHours();
    const intervalStart = hour - (hour % 4);
    date.setHours(intervalStart);

    const dateKey = date.toLocaleDateString("en-US", {
      month: "short",
      day: "numeric",
      hour: "numeric",
    });

    const functionName = curr.function.match(/{(.*)}/)?.[1];

    if (!functionName) return;

    functionSet.add(functionName);

    if (!groupedData[dateKey]) {
      groupedData[dateKey] = { date: dateKey };
    }

    groupedData[dateKey][functionName] =
      ((groupedData[dateKey][functionName] ?? 0) as number) + 1;
  });

  // Normalize data: Ensure all functions exist on all dates with 0 if missing
  return Object.values(groupedData).map(entry => {
    functionSet.forEach(func => {
      if (!(func in entry)) {
        entry[func] = 0; // Fill missing function data with 0
      }
    });
    return entry;
  });
};

export function InvocationsChart({ data = [] as Invocation[] }) {
  const chartData = processData(data);

  const functionList =
    chartData.length > 0
      ? Object.keys(chartData[0]!).filter(key => key !== "date")
      : [];

  return (
    <ChartContainer config={{}} className="h-[400px]">
      <BarChart data={chartData}>
        <CartesianGrid strokeDasharray="3 3" className="stroke-muted" />
        <XAxis dataKey="date" className="text-muted-foreground" />
        <YAxis className="text-muted-foreground" />
        <ChartTooltip
          content={({ active, payload }) => {
            if (active && payload?.length) {
              return (
                <div className="rounded-lg border bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60 p-3 shadow-md">
                  <div className="grid grid-cols-1 gap-3">
                    <div className="flex flex-col">
                      <span className="text-[0.70rem] uppercase text-muted-foreground font-medium">
                        Date
                      </span>
                      <span className="font-bold text-foreground">
                        {payload[0]!.payload.date}
                      </span>
                    </div>
                    {payload.map(entry => (
                      <div key={entry.name} className="flex flex-col">
                        <span className="text-[0.70rem] text-muted-foreground font-medium">
                          {entry.name}
                        </span>
                        <span
                          className="font-bold"
                          style={{ color: entry.color }}
                        >
                          {entry.value}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              );
            }
            return null;
          }}
        />
        {functionList.map((functionName, index) => (
          <Bar
            key={functionName}
            dataKey={functionName}
            fill={`hsl(${index * 50}, 70%, 50%)`} // Different colors
            stackId="a"
            radius={[4, 4, 0, 0]}
          />
        ))}
      </BarChart>
    </ChartContainer>
  );
}
