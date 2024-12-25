"use client";

import React, { useMemo, useState } from "react";
import {
  Grid,
  Paper,
  Typography,
  Box,
  Divider,
  List,
  ListItem,
  ListItemText,
  Button,
  CircularProgress,
} from "@mui/material";
import {
  InsertChart,
  CheckCircleOutline,
  ErrorOutline,
  RocketLaunch,
} from "@mui/icons-material";
import AddIcon from "@mui/icons-material/Add";
import CreateWorker from "@/components/create-worker";
import CustomModal from "@/components/CustomModal";
import useComponents from "@/lib/hooks/use-component";
import { useParams } from "next/navigation";
import { ComponentExport, WorkerFunction } from "@/types/api";
import useWorkers, { getStateFromWorkersData } from "@/lib/hooks/use-worker";

const Overview = () => {
  const [isOpen, setIsOpen] = useState(false);
  const { compId } = useParams<{ compId: string }>();

  const { components, isLoading: componentDataLoading } = useComponents(
    compId,
    "latest"
  );
  const [latestComponent] = components;

  const { workers, isLoading } = useWorkers(compId);

  const workerStats = useMemo(() => {
    const stats = getStateFromWorkersData(workers);
    return [
      {
        label: "Active Workers",
        value:
          Object.keys(stats)?.reduce(
            (acc, key) => (acc += key != "failed" ? stats[key] : 0),
            0
          ) || 0,
        icon: <CheckCircleOutline fontSize="large" />,
        isLoading: isLoading,
      },
      {
        label: "Running Workers",
        value: stats["running"] || 0,
        icon: <RocketLaunch fontSize="large" />,
        isLoading: isLoading,
      },
      {
        label: "Failed Workers",
        value: stats["failed"] || 0,
        icon: <ErrorOutline fontSize="large" />,
        isLoading: isLoading,
      },
    ];
  }, [workers]);

  const stats = useMemo(() => {
    return [
      {
        label: "Latest Component Version",
        value: latestComponent?.versionedComponentId?.version,
        icon: <InsertChart fontSize="large" />,
        isLoading: componentDataLoading,
      },
    ];
  }, [latestComponent]);

  const exports = useMemo(() => {
    const metaExports = (latestComponent?.metadata?.exports ||
      []) as ComponentExport[];
    return metaExports.flatMap(
      (expo: ComponentExport) =>
        expo?.functions?.map(
          (fun: WorkerFunction) => `${expo.name}.${fun.name}`
        ) || []
    );
  }, [latestComponent?.versionedComponentId?.version]);
  // const handleOpen = () => setIsOpen(true);
  const handleClose = () => setIsOpen(false);

  return (
    <Box sx={{ padding: 4, minHeight: "100vh" }}>
      <Box sx={{ display: "flex", justifyContent: "flex-end" }}>
        <Button
          variant="contained"
          startIcon={<AddIcon />}
          sx={{
            textTransform: "none",
            marginLeft: "2px",
            marginBottom: "8px",
          }}
          onClick={() => {
            setIsOpen(true);
          }}
        >
          New
        </Button>
      </Box>

      <Grid container spacing={4}>
        {/* Stats Section */}
        {[...stats, ...workerStats].map((stat, index) => (
          <Grid item xs={12} sm={6} md={3} key={index}>
            <Paper sx={{ padding: 2, textAlign: "center", bgcolor: "#1E1E1E" }}>
              <Box sx={{display:"flex", justifyContent:"space-between"}}>
                <Typography variant="body2">{stat.label}</Typography>
                 <Typography sx={{fontSize:"4px"}}>
                  {stat.icon}
                 </Typography>
                
              </Box>

              <Typography  variant="h5" sx={{ marginTop: 1,display:"flex" }}>
                {stat?.isLoading ? "Loading..." : stat.value}
              </Typography>
            </Paper>
          </Grid>
        ))}

        {/* Exports Section */}
        <Grid item xs={12} md={6}>
          <Paper sx={{ padding: 3, bgcolor: "#1E1E1E" }}>
            <Typography variant="h6">Exports</Typography>
            <Divider sx={{ bgcolor: "#424242", marginY: 1 }} />
            <List>
              {exports.map((item, index) => (
                <ListItem key={index} disableGutters>
                  <ListItemText primary={item} />
                </ListItem>
              ))}
            </List>
          </Paper>
        </Grid>

        {/* Worker Status */}
        <Grid item xs={12} md={6}>
          <Paper
            sx={{
              padding: 4,
              bgcolor: "#1E1E1E",
              height: 550,
            }}
          >
            <Typography variant="h6" sx={{ marginBottom: 2 }}>
              Worker Status
            </Typography>
            <Divider sx={{ bgcolor: "#424242", marginY: 1 }} />
            {workerStats.reduce((acc, stat) => acc + stat.value, 0) > 0 ? (
              <Box
                sx={{
                  display: "flex",
                  justifyContent: "center",
                  alignItems: "center",
                  position: "relative",
                  height: 200,
                  marginTop: 15,
                }}
              >
                <CircularProgress
                  variant="determinate"
                  value={
                    ((workerStats[0]?.value || 0) /
                      workerStats.reduce((acc, stat) => acc + stat.value, 0)) *
                    100
                  }
                  size={300}
                  thickness={10}
                  sx={{
                    color: "#36b4a5",
                    position: "absolute",
                  }}
                />
                {/* Center Text */}
                <Box
                  sx={{
                    position: "absolute",
                    textAlign: "center",
                    color: "#fff",
                  }}
                >
                  <Typography variant="h4" sx={{ fontWeight: "bold" }}>
                    {workerStats.reduce((acc, stat) => acc + stat.value, 0)}
                  </Typography>
                  <Typography variant="body2" sx={{ color: "#ccc" }}>
                    Total Workers
                  </Typography>
                </Box>
              </Box>
            ) : (
              <Typography>No workers found</Typography>
            )}
          </Paper>
        </Grid>
      </Grid>
      <CustomModal open={isOpen} onClose={handleClose} heading="Create Worker">
        <CreateWorker
          compId={compId}
          version={latestComponent?.versionedComponentId?.version}
        />
      </CustomModal>
    </Box>
  );
};

export default Overview;
