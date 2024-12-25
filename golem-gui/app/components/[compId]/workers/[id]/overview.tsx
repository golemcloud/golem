import { Worker } from "@/types/api";
import {
  CheckCircleOutline,
  RocketLaunch,
  ErrorOutline,
} from "@mui/icons-material";
import AccessTimeIcon from "@mui/icons-material/AccessTime";
import { Box, Button, Grid, Paper, Typography } from "@mui/material";
import React, { useMemo } from "react";
import { calculateHoursDifference, calculateSizeInMB } from "@/lib/utils";
import GenericCard from "@/components/ui/generic-card";

const cardStyle = {
  padding: 3,
  textAlign: "center",
  bgcolor: "#1E1E1E",
};

const Overview = ({ worker, isLoading }: { worker: Worker; isLoading: boolean }) => {
  const workerStats = useMemo(() => {
    return [
      {
        label: "Status",
        value: worker?.status,
        icon: <CheckCircleOutline fontSize="small" />,
      },
      {
        label: "Memory Usage",
        value: `${calculateSizeInMB(worker?.totalLinearMemorySize)} MB`,
        icon: <RocketLaunch fontSize="small" />,
      },
      {
        label: "Resource Count",
        value: `${worker?.ownedResources?.length ?? 0}`,
        icon: <ErrorOutline fontSize="small" />,
      },
      {
        label: "Created",
        value: calculateHoursDifference(worker?.createdAt),
        icon: <AccessTimeIcon fontSize="small" />,
      },
    ];
  }, [worker]);

  if (isLoading) {
    return <Typography>Loading...</Typography>;
  }

  return (
    <Box sx={{ marginBottom: 3, padding: 3, display: "flex", flexDirection: "column" }}>
      {worker ? (
        <Grid container spacing={4}>
          {/* Top Stats Section */}
          {workerStats.map((stat, index) => (
            <Grid item xs={12} sm={6} md={3} key={index}>
              <Paper sx={cardStyle}>
                <Box sx={{ display: "flex", justifyContent: "space-between" }}>
                  <Typography variant="body1">{stat.label}</Typography>
                  {stat.icon}
                </Box>
                <Box sx={{display:"flex"}}>
                <Typography variant="body1" sx={{ marginTop: 1 }}>
                  {stat.value || "N/A"}
                </Typography>
                </Box>
            
              </Paper>
            </Grid>
          ))}

         
          <Grid item xs={12}>
            <GenericCard
              title="Invocations"
              emptyMessage="No data available here"
            />
          </Grid>
          <Grid item xs={12}>
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
  );
};

export default Overview;
