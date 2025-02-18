import { EventMessage, InvocationStart, Worker } from "@lib/types/api";
import { Activity, Gauge, Cpu, Clock } from "lucide-react";
import { Box, Button, Grid2 as Grid, Paper, Stack, Typography, useMediaQuery, useTheme } from "@mui/material";
import React, { useMemo } from "react";
import { calculateHoursDifference, calculateSizeInMB } from "@lib/utils";
import { format } from "date-fns";

import {
  BarChart,
  Bar,
  XAxis,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import TerminalLogs from "./terminal";

// const cardStyle = {
//   padding: 3,
//   textAlign: "center",
//   bgcolor: "#1E1E1E",
// };

interface PayloadItem {
  dataKey: string; // Key in the data
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  payload: Record<string, any>; // The data object for this tooltip item
  color?: string; // Color for the bar/line
  fill?: string; // Fill color, if applicable
}

interface CustomTooltipProps {
  active?: boolean;
  payload?: PayloadItem[]; // The tooltip payload array
  graphKey: string; // Key to display in the tooltip
}

const CustomTooltip: React.FC<CustomTooltipProps> = ({ active, payload, graphKey }) => {
  if (active && payload && payload.length) {
    const barData = payload[0];
    return (
      <Paper
      elevation={3}
      sx={{
        padding: 1
      }}
      className="dark:bg-[#0a0a0a] bg-white dark:text-white"
      >
        <Typography variant="body1" fontStyle={"bold"} >{barData?.payload?.[graphKey]}</Typography>
        <Box padding={1}>
           {payload?.map((data)=>{
            return <Stack direction="row" gap={1} alignItems={"center"} key={data?.dataKey}>
              <Box sx={{height:10, width:10, backgroundColor:data?.color||data?.fill}}/>
                <Typography variant="caption">{data?.dataKey}{" "}{data?.payload?.[data.dataKey]}</Typography>
            </Stack>
           })}
        </Box>
      </Paper>
       
    );
  }
  return null;
};

const generateRandomColor = () => {
  const letters = "0123456789ABCDEF";
  let color = "#";
  for (let i = 0; i < 6; i++) {
    color += letters[Math.floor(Math.random() * 16)];
  }
  return color;
};

// const generateRandomColor = (theme: "light" | "dark") => {
//   const isDark = theme === "dark";
//   const randomChannel = () => Math.floor(Math.random() * 128) + (isDark ? 128 : 0); // Adjust range
//   const r = randomChannel(); // Red
//   const g = randomChannel(); // Green
//   const b = randomChannel(); // Blue
//   return `rgb(${r}, ${g}, ${b})`;
// };

const Overview = ({
  worker,
  isLoading,
  messages,
}: {
  worker: Worker;
  isLoading: boolean;
  messages: Array<EventMessage>;
}) => {
  const isMobile = useMediaQuery("(max-width: 640px)");
  //not sure theme.palette.mode is not giving right value. it is giving light always
  const theme = useTheme();
  const workerStats = useMemo(() => {
    return [
      {
        label: "Status",
        value: worker?.status,
        icon: <Activity fontSize="small" />,
      },
      {
        label: "Memory Usage",
        value: `${calculateSizeInMB(worker?.totalLinearMemorySize)} MB`,
        icon: <Gauge fontSize="small" />,
      },
      {
        label: "Resource Count",
        value: `${worker?.ownedResources?.length ?? 0}`,
        icon: <Cpu fontSize="small" />,
      },
      {
        label: "Created",
        value: calculateHoursDifference(worker?.createdAt),
        icon: <Clock fontSize="small" />,
      },
    ];
  }, [worker]);

  const invokeMessages = useMemo(() => {
    return Object.values(
      messages?.reduce<Record<string, InvocationStart["InvocationStart"]>>(
        (obj, message: EventMessage) => {
          if ("InvocationStart" in message) {
            const idempotency_key =
              message?.["InvocationStart"]?.idempotency_key;
            obj[idempotency_key] = message?.["InvocationStart"];
          }
          return obj;
        },
        {}
      ) || {}
    );
  }, [messages]);

  const uniquefunctions = new Set<string>();
  let graphKey = "live";
  const graphKeyMap = {
    daily: "",
    monthly: "",
    yearly: "",
  };
  console.log("thememode=======>", theme.palette.mode)
  // This can be improved further and needs to move to some other place where we can reuse it.
  const dataMap =
    invokeMessages?.reduce<Record<string, Record<string, number>>>(
      (stats, message: InvocationStart["InvocationStart"]) => {
        const currentDate = new Date(message.timestamp);
        const fullDate = format(currentDate, "dd-MM-yyyy HH:mm");
        const yearly = format(currentDate, "yyyy");
        const monthly = format(currentDate, "MMM");
        const daily = format(currentDate, "MMM dd");
        const live = format(currentDate, "HH:mm");
        const key = `${fullDate}`;
        if (graphKeyMap["monthly"] && graphKeyMap["monthly"] !== monthly) {
          graphKey = "monthly";
        }
        if (graphKeyMap["daily"] && graphKeyMap["daily"] !== daily) {
          graphKey = "daily";
        }
        if (graphKeyMap["yearly"] && graphKeyMap["yearly"] !== yearly) {
          graphKey = "yearly";
        }

        graphKeyMap["monthly"] = monthly;
        graphKeyMap["yearly"] = yearly;
        graphKeyMap["daily"] = daily;

        if (graphKey === "live") {
          stats[`live_${key}`] = stats[`live_${key}`] || {
            // name: message.function,
            yearly: yearly, // "Jan 2025"
            monthly: monthly, // "January"
            daily: daily, // "Jan 03"
            live: live,
          };
          stats[`live_${key}`][message.function] =
            (stats[`live_${key}`][message.function] || 0) + 1;
        }
        if (["live", "daily"].includes(graphKey)) {
          stats[`daily_${daily}`] = stats[`daily_${daily}`] || {
            yearly: yearly, // "Jan 2025"
            monthly: monthly, // "January"
            daily: daily, // "Jan 03"
            live: live,
          };
          stats[`daily_${daily}`][message.function] =
            (stats[`daily_${daily}`][message.function] || 0) + 1;
        }

        if (["live", "daily", "monthly"].includes(graphKey)) {
          stats[`monthly_${monthly}`] = stats[`monthly_${monthly}`] || {
            yearly: yearly, // "Jan 2025"
            monthly: monthly, // "January"
            daily: daily, // "Jan 03"
            live: live,
          };
          stats[`monthly_${monthly}`][message.function] =
            (stats[`monthly_${monthly}`][message.function] || 0) + 1;
        }

        if (["live", "daily", "monthly", "yearly"].includes(graphKey)) {
          stats[`yearly_${yearly}`] = stats[`yearly_${yearly}`] || {
            yearly: yearly, // "Jan 2025"
            monthly: monthly, // "January"
            daily: daily, // "Jan 03"
            live: live,
          };
          stats[`yearly_${yearly}`][message.function] =
            (stats[`yearly_${yearly}`][message.function] || 0) + 1;
        }
        uniquefunctions.add(message.function);
        return stats;
      },
      {}
    ) || {};

  const data = Object.keys(dataMap)
    .filter((key) => key.includes(graphKey))
    .map((key) => dataMap[key])
    .reverse();
  if (isLoading) {
    return <Typography>Loading...</Typography>;
  }

  return (
    <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
      <div className="mx-auto max-w-2xl lg:max-w-none py-4">
        <Box
          sx={{
            marginBottom: 3,
            display: "flex",
            flexDirection: "column",
          }}
        >
          {worker ? (
            <Grid container spacing={4}>
              {/* Top Stats Section */}

              {workerStats.map((stat, index) => (
                <Grid size={{ xs: 12, sm: 6, lg: 3 }} key={index}>
                  <Paper
                    sx={{ padding: 4, textAlign: "center", bgcolor: "#1E1E1E" }}
                    className="border"
                  >
                    <Box
                      sx={{ display: "flex", justifyContent: "space-between" }}
                    >
                      <Typography variant="body2">{stat.label}</Typography>
                      <Typography>{stat.icon}</Typography>
                    </Box>
                    <Typography
                      variant="h5"
                      sx={{ marginTop: 3, display: "flex" }}
                    >
                      {stat.value}
                    </Typography>
                  </Paper>
                </Grid>
              ))}

              <Grid size={12}>
                {/* <GenericCard
                  title="Invocations"
                  emptyMessage="No data available here"
                /> */}
                <Paper elevation={2} className="border rounded-sm p-5">
                  <Typography
                    variant="h6"
                    sx={{
                      marginBottom: 2,
                      fontWeight: "bold",
                      fontSize: "0.875rem",
                    }}
                  >
                    Invocations
                  </Typography>
                  {invokeMessages?.length === 0 ? (
                    <Box
                      sx={{
                        minHeight: "300px",
                        display: "flex",
                        justifyContent: "center",
                        alignItems: "center",
                        borderRadius: 2,
                        padding: 2,
                      }}
                    >
                      <Typography variant="body2" sx={{ color: "#AAAAAA" }}>
                        No data available here
                      </Typography>
                    </Box>
                  ) : (
                    <ResponsiveContainer
                      width="100%"
                      height="100%"
                      aspect={isMobile ? 500 / 200 : 500 / 150}
                    >
                      <BarChart
                        data={data}
                        margin={{
                          top: 20,
                          right: 30,
                          left: 20,
                          bottom: 5,
                        }}
                        barCategoryGap={Math.max(1, 100 / data.length)}
                      >
                        <XAxis
                          dataKey={graphKey}
                          interval="preserveStartEnd"
                          tick={{ fontSize: 12 }}
                          tickSize={5}
                          tickMargin={5}
                          minTickGap={5}
                        />
                        <Tooltip content={<CustomTooltip graphKey={graphKey}/>}/>
                        {Array.from(uniquefunctions)?.map((bar) => {
                          return (
                            <Bar
                              dataKey={bar}
                              stackId="a"
                              fill={generateRandomColor()} // Cycle through the colors array
                              key={bar}
                            />
                          );
                        })}
                      </BarChart>
                    </ResponsiveContainer>
                  )}
                </Paper>
              </Grid>
              <Grid size={12}>
                <Paper elevation={2} className="border rounded-sm p-5">
                  <Typography
                    variant="h6"
                    sx={{
                      marginBottom: 2,
                      fontWeight: "bold",
                      fontSize: "0.875rem",
                    }}
                  >
                    Terminal
                  </Typography>
                  {invokeMessages?.length === 0 ? (
                    <Box
                      sx={{
                        minHeight: "300px",
                        display: "flex",
                        justifyContent: "center",
                        alignItems: "center",
                        borderRadius: 2,
                        padding: 2,
                      }}
                    >
                      <Typography variant="body2" sx={{ color: "#AAAAAA" }}>
                        No data available here
                      </Typography>
                    </Box>
                  ) : (
                    <Box
                      sx={{
                        minHeight: "300px",
                        borderRadius: 2,
                        padding: 2,
                      }}
                    >
                      <TerminalLogs messages={messages} />
                    </Box>
                  )}
                </Paper>
              </Grid>
            </Grid>
          ) : (
            <Box
              className="dark:bg-gray-800 bg-[#E3F2FD] dark:text-white text-black"
              sx={{
                flex: 1,
                display: "flex",
                justifyContent: "center",
                alignItems: "center",
                flexDirection: "column",
                padding: 3,
                borderRadius: 1,
              }}
            >
              {/* <Typography variant="h6" sx={{ mb: 1 }}>
                No Workers Found
              </Typography>
              <Typography variant="body2" sx={{ mb: 2 }}>
                Contact Support
              </Typography> */}
              <Button
                variant="contained"
                sx={{
                  "&:hover": { backgroundColor: "#0039CB" },
                }}
              >
                Retry
              </Button>
            </Box>
          )}
        </Box>
      </div>
    </div>
  );
};

export default Overview;
