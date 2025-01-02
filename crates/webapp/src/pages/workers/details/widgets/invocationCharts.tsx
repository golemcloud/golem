import {
  Bar,
  BarChart,
  ResponsiveContainer,
  XAxis,
  YAxis,
  Tooltip,
} from "recharts";

const invocationData = [
  { time: "23:00", value: 80 },
  { time: "01:00", value: 0 },
  { time: "03:00", value: 0 },
  { time: "05:00", value: 0 },
  { time: "07:00", value: 0 },
  { time: "09:00", value: 0 },
  { time: "11:00", value: 0 },
  { time: "13:00", value: 0 },
  { time: "15:00", value: 0 },
  { time: "17:00", value: 0 },
  { time: "19:00", value: 0 },
  { time: "21:00", value: 0 },
];

export function InvocationsChart() {
  return (
    <ResponsiveContainer width="100%" height={200}>
      <BarChart data={invocationData}>
        <XAxis
          dataKey="time"
          stroke="#888888"
          fontSize={12}
          tickLine={false}
          axisLine={false}
        />
        <YAxis
          stroke="#888888"
          fontSize={12}
          tickLine={false}
          axisLine={false}
          tickFormatter={(value) => `${value}`}
        />
        <Tooltip />
        <Bar dataKey="value" fill="hsl(var(--primary))" radius={[4, 4, 0, 0]} />
      </BarChart>
    </ResponsiveContainer>
  );
}
