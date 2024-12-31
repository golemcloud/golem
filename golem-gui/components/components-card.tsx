"use client";
import React from "react";
import { Dropdown } from "./ui/dropdown-button";
import { Box, Card, CardContent, Typography } from "@mui/material";

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
          }}
        >
          <Typography variant="h6" component="div">
            {title}
          </Typography>
          {Dropdown(workloads)}
        </Box>
        <Box
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
            <Typography variant="body2">0 ▶</Typography>
          </Box>
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Idle
            </Typography>
            <Typography variant="body2">0 ⏸</Typography>
          </Box>
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Suspended
            </Typography>
            <Typography variant="body2">0 ⏹</Typography>
          </Box>
          <Box>
            <Typography
              className="text-muted-foreground"
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Failed
            </Typography>
            <Typography variant="body2">0 ⚠</Typography>
          </Box>
        </Box>

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
