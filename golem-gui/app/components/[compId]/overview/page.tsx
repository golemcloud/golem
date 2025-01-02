"use client";

import React, { useEffect, useMemo, useState } from "react";
import {
  Grid2 as Grid,
  Paper,
  Typography,
  Box,
  Divider,
  List,
  ListItem,
  ListItemText,
  CircularProgress,
} from "@mui/material";
import {
  InsertChart,
  CheckCircleOutline,
  ErrorOutline,
  RocketLaunch,
} from "@mui/icons-material";
import CreateWorker from "@/components/create-worker";
import CustomModal from "@/components/CustomModal";
import useComponents from "@/lib/hooks/use-component";
import { useParams, useRouter } from "next/navigation";
import { ComponentExport, WorkerFunction } from "@/types/api";
import useWorkers, { getStateFromWorkersData } from "@/lib/hooks/use-worker";
import SecondaryHeader from "@/components/ui/secondary-header";

const Overview = () => {
  const [isOpen, setIsOpen] = useState(false);
  const { compId } = useParams<{ compId: string }>();
  const router = useRouter();

  const { components, isLoading: componentDataLoading } = useComponents(
    compId,
    "latest"
  );
  const [latestComponent] = components;

  const { workers, isLoading } = useWorkers(compId);

  const getNewSearchParams = (key:string, value:string[]|string|number)=>{
  const searchParams = new URLSearchParams();
  if (value && Array.isArray(value)) {
    searchParams.set(key, JSON.stringify(value));
  }
  return searchParams.toString();
  }

  const workerStats = useMemo(() => {
    const stats = getStateFromWorkersData(workers);
    return [
      {
        label: "Active Workers",
        value:
          Object.keys(stats)?.reduce(
            (acc, key) => (acc += key !== "failed" ? stats[key] : 0),
            0
          ) || 0,
        icon: <CheckCircleOutline fontSize="small" />,
        isLoading: isLoading,
        handleClick:(()=>router.push(`/components/${compId}/workers?${getNewSearchParams('workerStatus', ["Running", "Idle", "Suspended"])}`))
      },
      {
        label: "Running Workers",
        value: stats["running"] || 0,
        icon: <RocketLaunch fontSize="small" />,
        isLoading: isLoading,
        handleClick:(()=>router.push(`/components/${compId}/workers?${getNewSearchParams('workerStatus', ["Running"])}`))

      },
      {
        label: "Failed Workers",
        value: stats["failed"] || 0,
        icon: <ErrorOutline fontSize="small" />,
        isLoading: isLoading,
        handleClick:(()=>{router.push(`/components/${compId}/workers?${getNewSearchParams('workerStatus', ["Failed"])}`)})

      },
    ];
  }, [workers]);

  const stats = useMemo(() => {
    return [
      {
        label: "Latest Component Version",
        value: `v${latestComponent?.versionedComponentId?.version}`,
        icon: <InsertChart fontSize="small" />,
        isLoading: componentDataLoading,
        handleClick:()=>{}
      },
    ];
  }, [latestComponent]);

  const exports = useMemo(() => {
    const metaExports = (latestComponent?.metadata?.exports || []) as ComponentExport[];
    return metaExports.flatMap((expo: ComponentExport) =>
      "functions" in expo
        ? expo.functions?.map((fun: WorkerFunction) => `${expo.name}.${fun.name}`)
        : expo.name
    );
  }, [latestComponent?.versionedComponentId?.version]);

  const handleClose = () => setIsOpen(false);

  const totalWorkers = workerStats.reduce((acc, stat) => acc + stat.value, 0);
  const activeWorkers = workerStats[0]?.value || 0;

  const [progress, setProgress] = useState(0);

useEffect(() => {
  if (totalWorkers > 0) {
    const targetProgress = (activeWorkers / totalWorkers) * 100; // Target progress percentage
    const duration = 2000; // Total duration in milliseconds
    const startTime = performance.now();

    const animate = (currentTime: number) => {
      const elapsedTime = currentTime - startTime;
      const progressValue = Math.min((elapsedTime / duration) * targetProgress, targetProgress);

      setProgress(progressValue);

      if (elapsedTime < duration) {
        requestAnimationFrame(animate); // Continue animation
      }
    };

    requestAnimationFrame(animate); // Start animation
  }
}, [totalWorkers, activeWorkers]);

  return (
    <>
      <SecondaryHeader onClick={() => setIsOpen(true)} variant="components" />
      <div className="mx-auto max-w-7xl px-6 lg:px-8">
        <div className="mx-auto max-w-2xl lg:max-w-none py-4">
          <Grid container spacing={4}>
            {[...stats, ...workerStats].map((stat, index) => (
              <Grid size={{ xs: 12, sm: 6, lg: 3 }} key={index} onClick={stat?.handleClick}>
                <Paper sx={{ padding: 4, textAlign: "center", bgcolor: "#1E1E1E" }} className="border">
                  <Box sx={{ display: "flex", justifyContent: "space-between" }}>
                    <Typography variant="body2">{stat.label}</Typography>
                    <Typography>{stat.icon}</Typography>
                  </Box>
                  <Typography variant="h5" sx={{ marginTop: 3, display: "flex" }}>
                    {stat?.isLoading ? "Loading..." : stat.value}
                  </Typography>
                </Paper>
              </Grid>
            ))}

            {/* Exports Section */}
            <Grid size={{ xs: 12, md: 4 }}>
              <Paper sx={{ padding: 3, bgcolor: "#1E1E1E", minHeight: 550 }} className="border">
                <Typography variant="h6">Exports</Typography>
                <Divider sx={{ bgcolor: "#555", marginY: 1 }} />
                <List>
                  {exports.slice(0,13).map((item, index) => (
                    <ListItem key={index} sx={{

                    }}
                    divider
                    >
                      <ListItemText primary={item} />
                    </ListItem>
                  ))}
                </List>
              </Paper>
            </Grid>

            {/* Worker Status */}
            <Grid size={{ xs: 12, md: 8 }}>
              <Paper sx={{ padding: 4, bgcolor: "#1E1E1E", minHeight:550 }} className="border">
                <Typography variant="h6">Worker Status</Typography>
                <Divider sx={{ bgcolor: "#555", marginY: 1 }} />
                {totalWorkers > 0 ? (
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
                      value={progress}
                      size={300}
                      thickness={10}
                      sx={{ position: "absolute" }}
                    />
                    <Box sx={{ position: "absolute", textAlign: "center" }}>
                      <Typography variant="h4" sx={{ fontWeight: "bold" }}>
                        {totalWorkers}
                      </Typography>
                      <Typography variant="body2">Total Workers</Typography>
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
              onSuccess={handleClose}
            />
          </CustomModal>
        </div>
      </div>
    </>
  );
};

export default Overview;
