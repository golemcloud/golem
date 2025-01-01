"use client";
import React from "react";
import { Box, Card, CardContent, Typography, Stack } from "@mui/material";
import { Dropdown } from "./ui/dropdown-button";
import { Worker } from "@/types/api";
import { calculateHoursDifference, calculateSizeInMB } from "@/lib/utils";


export default function WorkerInfoCard({ worker, onClick }: { worker: Worker; onClick: () => void }) {
  const workerInfo = [
    `v${worker.componentVersion}`,
    `Env: ${Object.values(worker.env).length}`,
    `Args: ${worker.args.length}`,
  ];

  const workloads = [
    { route: `/components/${worker.workerId.workerName}/overview`, value: "View Details" },
  ];

  return (
    <Card
      sx={{
        borderRadius: 2,
        minWidth: "200px",
        "&:hover": { cursor: "pointer", boxShadow: "0px 5px 10px 0px #666" },
      }}
      className="flex-1 border"
      onClick={onClick}
    >
      <CardContent>
        <Box
          sx={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
          }}
        >
          <Typography variant="h6" component="div">
            {worker.workerId.workerName}
          </Typography>
          {Dropdown(workloads)}
        </Box>

  
        {/* Worker Info */}
        <Box
          sx={{
            display: "flex",
            width: "80%",
            justifyContent: "space-between",
            alignItems: "center",
            mt: 2,
            mb: 2,
          }}
        >
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Status
            </Typography>
            <Typography variant="body2">{worker.status} </Typography>
          </Box>
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Memory
            </Typography>
            <Typography variant="body2">{`${calculateSizeInMB(worker.totalLinearMemorySize)} MB`}</Typography>
          </Box>
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Pending Invocation
            </Typography>
            <Typography variant="body2">{worker.pendingInvocationCount}</Typography>
          </Box>
          <Box>

            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Resources
            </Typography>
            <Typography variant="body2">{Object.values(worker.ownedResources).length}</Typography>
          </Box>
        </Box>
        <Box sx={{ display: "flex", gap: 1, alignItems: "center", flexWrap: "wrap" }}>
          {workerInfo.map((info, index) => (
            <Typography
              key={index}
              variant="body2"
              className="border text-muted-foreground px-2 rounded-md"
            >
              {info}
            </Typography>
          ))}
            <Typography variant="body2" className="ml-auto text-muted-foreground ">
            {calculateHoursDifference(worker.createdAt)}
          </Typography>
        </Box>
      </CardContent>
    </Card>
  );
}
