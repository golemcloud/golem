"use client";

import React, { useMemo, useState } from "react";
import {
  Box,
  Button,
  Typography,
  Tab,
  Tabs,
  Grid,
  Paper,
} from "@mui/material";
import {useWorker} from "@/lib/hooks/use-worker";
import { useParams } from "next/navigation";
import { CheckCircleOutline, ErrorOutline, RocketLaunch } from "@mui/icons-material";

const WorkerListWithDropdowns = () => {
  const [activeTab, setActiveTab] = useState(0);
  
  //TO DO: let show filters in url so that user can share the url to others.
  const { compId } = useParams<{ compId: string }>();
  const {id: workerName} = useParams<{id:string}>();  
  //need to integrate the filter logic here. and pagination or scroll on load needs to implemented or addd show more at the end on click we need to next set of data
  const { worker, isLoading } = useWorker(compId, workerName);
  const workerStats = useMemo(()=>{
      return       [{
        label: "Status",
        value: worker?.status,
        icon: <CheckCircleOutline fontSize="large" />,
        isLoading: isLoading
      },
      {
        label: "Memory Usage",
        value: `${worker?.totalLinearMemorySize}`,
        icon: <RocketLaunch fontSize="large" />,
        isLoading: isLoading
      },
      {
        label: "Resource Count",
        value: `${worker?.ownedResources?.length ?? 0}`,
        icon: <ErrorOutline fontSize="large" />,
        isLoading: isLoading
      }]

  }, [worker])
 

  const handleTabChange = (event: React.SyntheticEvent, newValue: number) => {
      setActiveTab(newValue);
  };

  return (
    <>
    <Tabs
            value={activeTab}
            onChange={handleTabChange}
            aria-label="Worker Settings Tabs"
            textColor="inherit"
          >
            <Tab label="Overview" />
            <Tab label="Live" />
            <Tab label="Environment" />
            <Tab label="Files" />
            <Tab label="Manage" />
          </Tabs>
      <Box
        sx={{
          marginBottom: 3,
          padding: 3,
          display: "flex",
          flexDirection: "column",
        }}
      >
        {/* No Workers Found */}
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

        <Box>
          {!isLoading && worker && (
              <Grid container spacing={4}>
        {/* Stats Section */}
          {workerStats.map((stat, index) => (
                    <Grid item xs={12} sm={6} md={3} key={index}>
                      <Paper sx={{ padding: 3, textAlign: "center", bgcolor: "#1E1E1E" }}>
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
      </Box>
    </>
  );
};

export default WorkerListWithDropdowns;
