"use client";

import React, { useMemo, useState } from "react";
import {
  Grid2 as Grid,
  Paper,
  Typography,
  Box,
  Divider,
  List,
  ListItem,
  ListItemText,
} from "@mui/material";
import {
  InsertChart,
  CheckCircleOutline,
  ErrorOutline,
  RocketLaunch,
} from "@mui/icons-material";
import { PieChart, Pie,Cell, ResponsiveContainer } from "recharts";
import CreateWorker from "@/components/create-worker";
import CustomModal from "@/components/CustomModal";
import useComponents from "@/lib/hooks/use-component";
import {  useRouter } from "next/navigation";
import { ComponentExport, WorkerFunction } from "@/types/api";
import useWorkers, { getStateFromWorkersData } from "@/lib/hooks/use-worker";
import SecondaryHeader from "@/components/ui/secondary-header";
import ErrorBoundary from "@/components/erro-boundary";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

const Overview = () => {
  const [isOpen, setIsOpen] = useState(false);
  const { compId } = useCustomParam();
  const router = useRouter();

  const { components, isLoading: componentDataLoading, error } = useComponents(
    compId,
    "latest"
  );
  const [latestComponent] = components;

  const { workers, isLoading, error:workerError } = useWorkers(compId);

  const getNewSearchParams = (
    key: string,
    value: string[] | string | number
  ) => {
    const searchParams = new URLSearchParams();
    if (value && Array.isArray(value)) {
      searchParams.set(key, JSON.stringify(value));
    }
    return searchParams.toString();
  };

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
        handleClick: () =>
          router.push(
            `/components/${compId}/workers?${getNewSearchParams(
              "workerStatus",
              ["Running", "Idle", "Suspended"]
            )}`
          ),
      },
      {
        label: "Running Workers",
        value: stats["running"] || 0,
        icon: <RocketLaunch fontSize="small" />,
        isLoading: isLoading,
        handleClick: () =>
          router.push(
            `/components/${compId}/workers?${getNewSearchParams(
              "workerStatus",
              ["Running"]
            )}`
          ),
      },
      {
        label: "Failed Workers",
        value: stats["failed"] || 0,
        icon: <ErrorOutline fontSize="small" />,
        isLoading: isLoading,
        handleClick: () => {
          router.push(
            `/components/${compId}/workers?${getNewSearchParams(
              "workerStatus",
              ["Failed"]
            )}`
          );
        },
      },
    ];
  }, [workers, isLoading, router, compId]);

  const stats = useMemo(() => {
    return [
      {
        label: "Latest Component Version",
        value: error ?  "0" : `v${latestComponent?.versionedComponentId?.version}`,
        icon: <InsertChart fontSize="small" />,
        isLoading: componentDataLoading,
        handleClick: () => {},
      },
    ];
  }, [error, latestComponent?.versionedComponentId?.version, componentDataLoading]);

  const exports = useMemo(() => {
    const metaExports = (latestComponent?.metadata?.exports ||
      []) as ComponentExport[];
    return metaExports.flatMap((expo: ComponentExport) =>
      "functions" in expo
        ? expo.functions?.map(
            (fun: WorkerFunction) => `${expo.name}.${fun.name}`
          )
        : expo.name
    );
  }, [latestComponent?.versionedComponentId?.version]);

  const handleClose = () => setIsOpen(false);

  const totalWorkers = workerStats.reduce((acc, stat) => acc + stat.value, 0);
  const colors = ["#2a9d90", "#FFC107", "#F44336"];

  const pieData = workerStats.map((stat, index) => ({
    name: stat.label,
    value: stat.value,
    color: colors[index],
  }));

  const isSameError = useMemo(()=>{
    return error && error == workerError 
  },[error, workerError])

  return (
    <>
      <SecondaryHeader onClick={() => setIsOpen(true)} variant="components" />
      {isSameError && <ErrorBoundary message={error}/>}
      <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
        <div className="mx-auto max-w-2xl lg:max-w-none py-4">
          <ErrorBoundary message={!isSameError ? error: ""}/>
          <Grid container spacing={4}>
            {[...stats, ...workerStats].map((stat, index) => (
              <Grid
                size={{ xs: 12, sm: 6, lg: 3 }}
                key={index}
                onClick={stat?.handleClick}
              >
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
                    {stat?.isLoading ? "Loading..." : stat.value}
                  </Typography>
                </Paper>
              </Grid>
            ))}

            {/* Exports Section */}
            <Grid size={{ xs: 12, md: 4 }}>
              <Paper
                sx={{ bgcolor: "#1E1E1E", minHeight: 550 }}
                className="border"
              >
                <Typography variant="h6" className="m-5">
                  Exports
                </Typography>
                <Divider className="my-1 bg-border" />
                <ErrorBoundary message={!isSameError ? error: ""}>
                <List className="px-7">
                  {exports.slice(0, 13).map((item, index) => (
                    <ListItem key={index} divider className="border-border">
                      <ListItemText primary={item} />
                    </ListItem>
                  ))}
                </List>
                </ErrorBoundary>
              </Paper>
            </Grid>

            {/* Worker Status */}
            <Grid size={{ xs: 12, md: 8 }}>
              <Paper
                sx={{ bgcolor: "#1E1E1E", minHeight: 550 }}
                className="border"
              >
                <Typography variant="h6" className="m-5">
                  Worker Status
                </Typography>
                <Divider className="my-1 bg-border" />
                {!isSameError  && <ErrorBoundary message={workerError}/>}
                {totalWorkers > 0 ? (
                  <Box
                    sx={{
                      display: "flex",
                      justifyContent: "center",
                      alignItems: "center",
                      position: "relative",
                      height: 250,
                      marginTop: 15,
                    }}
                  >
                    <ResponsiveContainer width="100%" height="100%">
                      <PieChart>
                        <Pie
                          data={pieData}
                          dataKey="value"
                          cx="50%"
                          cy="50%"
                          innerRadius={60}
                          outerRadius={120}
                          labelLine={false}
                        >
                          {pieData.map((entry, index) => (
                            <Cell key={`cell-${index}`} fill={entry.color} />
                          ))}
                        </Pie>
                      </PieChart>
                    </ResponsiveContainer>

                    <Box sx={{ position: "absolute", textAlign: "center" }}>
                      <Typography variant="h4" sx={{ fontWeight: "bold" }}>
                        {totalWorkers}
                      </Typography>
                      <Typography variant="body2">Total Workers</Typography>
                    </Box>
                  </Box>
                ) : (
                  <Typography className="mt-5 ml-5">
                    No workers found
                  </Typography>
                )}
              </Paper>
            </Grid>
          </Grid>
          <CustomModal
            open={isOpen}
            onClose={handleClose}
            heading="Create Worker"
          >
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
