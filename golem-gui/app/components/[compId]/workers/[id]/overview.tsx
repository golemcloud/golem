import { Worker } from "@/types/api";
import { CheckCircleOutline, RocketLaunch, ErrorOutline } from "@mui/icons-material";
import { Box, Button, Grid, Paper, Typography } from "@mui/material";
import React, { useMemo } from "react";

export default function Overview({
  worker,
  isLoading,
}: {
  worker: Worker;
  isLoading: boolean;
}) {
  const workerStats = useMemo(() => {
    return [
      {
        label: "Status",
        value: worker?.status,
        icon: <CheckCircleOutline fontSize="large" />,
        isLoading: isLoading,
      },
      {
        label: "Memory Usage",
        value: `${worker?.totalLinearMemorySize}`,
        icon: <RocketLaunch fontSize="large" />,
        isLoading: isLoading,
      },
      {
        label: "Resource Count",
        value: `${worker?.ownedResources?.length ?? 0}`,
        icon: <ErrorOutline fontSize="large" />,
        isLoading: isLoading,
      },
    ];
  }, [worker]);
  return (
    <Box
      sx={{
        marginBottom: 3,
        padding: 3,
        display: "flex",
        flexDirection: "column",
      }}
    >
      {/* No Workers Found */}

      <Box>
        {!isLoading && worker && (
          <Grid container spacing={4}>
            {/* Stats Section */}
            {workerStats.map((stat, index) => (
              <Grid item xs={12} sm={6} md={3} key={index}>
                <Paper
                  sx={{
                    padding: 3,
                    textAlign: "center",
                    bgcolor: "#1E1E1E",
                  }}
                >
                  {stat.icon}
                  <Typography variant="h5" sx={{ marginTop: 1 }}>
                    {stat?.isLoading ? "Loading..." : stat.value}
                  </Typography>
                  <Typography variant="body1">{stat.label}</Typography>
                </Paper>
              </Grid>
            ))}

            {/* Exports Section */}
            <Grid item xs={12} md={6}>
              <Paper sx={{ padding: 3, bgcolor: "#1E1E1E" }}>
                {/* <List>
              {exports.map((item, index) => (
                <ListItem key={index} disableGutters>
                  <ListItemText primary={item} />
                </ListItem>
              ))}
            </List> */}
              </Paper>
            </Grid>
          </Grid>
        )}
      </Box>
      {!isLoading && !worker && (
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
}
