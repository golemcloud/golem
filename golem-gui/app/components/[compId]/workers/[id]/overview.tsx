import { Worker } from "@/types/api";
import { Activity, Gauge, Cpu, Clock } from "lucide-react";
import { Box, Button, Grid2 as Grid, Paper, Typography } from "@mui/material";
import React, { useMemo } from "react";
import { calculateHoursDifference, calculateSizeInMB } from "@/lib/utils";
import GenericCard from "@/components/ui/generic-card";

const cardStyle = {
  padding: 3,
  textAlign: "center",
  bgcolor: "#1E1E1E",
};

const Overview = ({
  worker,
  isLoading,
}: {
  worker: Worker;
  isLoading: boolean;
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

  if (isLoading) {
    return <Typography>Loading...</Typography>;
  }

  return (
    <div className="mx-auto max-w-7xl px-6 lg:px-8">
      <div className="mx-auto max-w-2xl lg:max-w-none py-4">
        <Box
          sx={{
            marginBottom: 3,
            padding: 3,
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
                <GenericCard
                  title="Invocations"
                  emptyMessage="No data available here"
                />
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
              <Typography variant="h6" sx={{ mb: 1 }}>
                No Workers Found
              </Typography>
              <Typography variant="body2" sx={{ mb: 2 }}>
                Contact Support
              </Typography>
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
