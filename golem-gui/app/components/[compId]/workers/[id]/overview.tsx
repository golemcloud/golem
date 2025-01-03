import { EventMessage, InvocationStart, Worker } from "@/types/api";
import { Activity, Gauge, Cpu, Clock } from "lucide-react";
import { Box, Button, Grid2 as Grid, Paper, Typography } from "@mui/material";
import React, { useMemo } from "react";
import { calculateHoursDifference, calculateSizeInMB } from "@/lib/utils";
import GenericCard from "@/components/ui/generic-card";
import { format } from "date-fns";

import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer,
} from "recharts";

// const cardStyle = {
//   padding: 3,
//   textAlign: "center",
//   bgcolor: "#1E1E1E",
// };

//TO DO: for now usng this harcoded colors. we need to maintian random color generator
const colors = [
  "#8884d8",
  "#82ca9d",
  "#ffc658",
  "#ff7f50",
  "#a83279",
  "#50c878",
];

const Overview = ({
  worker,
  isLoading,
  messages,
}: {
  worker: Worker;
  isLoading: boolean;
  messages: Array<EventMessage>;
}) => {
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
  }
  const dataMap =
    invokeMessages?.reduce<Record<string, Record<string, number>>>(
      (stats, message: InvocationStart["InvocationStart"]) => {
        const currentDate = new Date(message.timestamp);
        const fullDate = format(currentDate, "dd-MM-yyyy HH:mm");
        const yearly= format(currentDate, "yyyy")
        const monthly= format(currentDate, "MMM")
        const daily= format(currentDate, "MMM dd")
        const live= format(currentDate, "HH:mm") 
        const key = `${fullDate}`;
        if(graphKeyMap["monthly"] && graphKeyMap["monthly"]!==monthly){
          graphKey = "monthly"
        }
        if(graphKeyMap["daily"] && graphKeyMap["daily"]!==daily){
          graphKey = "daily"
        }
        if(graphKeyMap["yearly"] && graphKeyMap["yearly"]!==yearly){
          graphKey = "yearly"
        }

        graphKeyMap["monthly"] = monthly;
        graphKeyMap["yearly"] = yearly;
        graphKeyMap["daily"] = daily;

        stats[key] = stats[key] || {
          name: message.function,
          yearly: yearly, // "Jan 2025"
          monthly: monthly,   // "January"
          daily: daily,   // "Jan 03"
          live: live,   
        };
        stats[key][message.function] = (stats[key][message.function] || 0) + 1;
        uniquefunctions.add(message.function);

        return stats;
      },
      {}
    ) || {};

  const data = Object.values(dataMap);
  if (isLoading) {
    return <Typography>Loading...</Typography>;
  }

  return (
    <div className="mx-auto max-w-7xl px-6 lg:px-8">
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
                {/* <ResponsiveContainer width="100%" height="100%"> */}
                <Paper>
                  <BarChart
                    width={1200}
                    height={300}
                    data={data}
                    margin={{
                      top: 20,
                      right: 30,
                      left: 20,
                      bottom: 5,
                    }}
                    barCategoryGap={Math.max(1, 100 / data.length)}
                  >
                    <XAxis dataKey={graphKey} />
                    <YAxis />
                    <Tooltip />
                    {Array.from(uniquefunctions)?.map((bar, index) => {
                      return (
                        <Bar
                          dataKey={bar}
                          stackId="a"
                          fill={colors[index % colors.length]} // Cycle through the colors array
                          key={bar}
                        />
                      );
                    })}
                  </BarChart>
                </Paper>

                {/* </ResponsiveContainer> */}
              </Grid>
              <Grid size={12}>
                <GenericCard
                  title="Terminal"
                  emptyMessage="No data available here"
                />
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
