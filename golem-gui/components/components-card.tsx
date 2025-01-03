"use client";
import React, { useMemo } from "react";
import { Dropdown } from "./ui/dropdown-button";
import { Box, Card, CardContent, Typography } from "@mui/material";
import { useWorkerFind } from "@/lib/hooks/use-worker";
import { Worker } from "@/types/api";

interface ComponentInfoCardProps {
  title: string;
  time: string;
  version: number;
  exports: number;
  size: string;
  componentType: string;
  id: string;
  onClick?: () => void;
}


export function ComponentWorkerInfo({compId}:{compId:string}){
  // TODO ADD loader and error handling
  const {data: workers, error, isLoading} = useWorkerFind(compId, 15);
  const stats = useMemo(()=>{
    return workers?.reduce<Record<string, number>>((obj:Record<string, number>, worker: Worker)=>{
      obj[worker.status] = (obj[worker.status] || 0) + 1
      return obj;
    },{})

  }, [workers])
  

  return <Box
          sx={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            mt: 1,
            mb: 2,
          }}
        >
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Running
            </Typography>
            <Typography variant="body2">{stats["Running"] || 0} ▶</Typography>
          </Box>
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Idle
            </Typography>
            <Typography variant="body2">{stats["Idle"] || 0}⏸</Typography>
          </Box>
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Suspended
            </Typography>
            <Typography variant="body2">{stats["Suspended"] || 0} ⏹</Typography>
          </Box>
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Failed
            </Typography>
            <Typography variant="body2">{stats["Failed"] || 0} ⚠</Typography>
          </Box>
        </Box>

}

const ComponentInfoCard = ({
  title,
  time,
  version,
  exports,
  size,
  componentType,
  id,
  onClick,
}: ComponentInfoCardProps) => {
  const cardInfo = [`${exports} Exports`, `${size} MB`, componentType];
  const workloads = [
    { route: `/components/${id}/overview`, value: "New Worker" },
    { route: `/components/${id}/settings`, value: "Settings" },
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
            gap:2
          }}
        >
          <Typography variant="h6" component="div"
           sx={{
            overflow: "hidden", // Ensures overflow content is hidden
            textOverflow: "ellipsis", // Adds an ellipsis when text overflows
            whiteSpace: "nowrap", // Prevents text wrapping to a new line
            fontWeight: 500
          }}
          >
            {title}
          </Typography>
          {Dropdown(workloads)}
        </Box>
        {/* this needs improvment. query all componenets worker info is not feasable.*/}
        <ComponentWorkerInfo compId={id}/>
        <Box sx={{ display: "flex", gap: 1, alignItems: "center" }}>
          <Typography className=" bg-button_bg border border-button_border px-2  rounded-sm text-sm">
            v{version}
          </Typography>
          {cardInfo.map((info, index) => (
            <Typography
              key={index}
              variant="body2"
              className="border text-muted-foreground px-2 rounded-md"
            >
              {info}
            </Typography>
          ))}

          <Typography variant="body2" className="ml-5 text-muted-foreground">
            {time}
          </Typography>
        </Box>
      </CardContent>
    </Card>
  );
};

export default ComponentInfoCard;
