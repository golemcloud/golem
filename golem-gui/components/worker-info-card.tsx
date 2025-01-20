"use client";
import React from "react";
import { Box, Card, CardContent, Typography } from "@mui/material";
import { Dropdown } from "./ui/dropdown-button";
import { Worker } from "@/types/api";
import { calculateHoursDifference, calculateSizeInMB } from "@/lib/utils";
import { useCustomParam } from "@/lib/hooks/use-custom-param";


export default function WorkerInfoCard({ worker, onClick }: { worker: Worker; onClick: () => void }) {
  const { compId } = useCustomParam();
  const workerInfo = [
    `v${worker.componentVersion}`,
    `Env: ${Object.values(worker.env).length}`,
    `Args: ${worker.args.length}`,
  ];

  const workloads = [
    { route: `/components/${compId}/workers/${worker.workerId.workerName}`, value: "View Details" },
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
        <Box className="flex sm:w-[80%] w-[100%] items-center justify-between mt-2 mb-2 gap-2">
          <Box className="w-fit">
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
            >
              Status
            </Typography>
            <Typography variant="body2"
            className={worker.status==="Failed"?"text-red-500":""}
            >{worker.status} </Typography>
          </Box>

          <Box className="w-fit">
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
            >
              Memory
            </Typography>
            <Typography variant="body2">{`${calculateSizeInMB(worker.totalLinearMemorySize)} MB`}</Typography>
          </Box>

          <Box className="w-fit">
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
            >
              Pending Invocation
            </Typography>
            <Typography variant="body2">{worker.pendingInvocationCount}</Typography>
          </Box>

          <Box className="w-fit">
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
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
