"use client";
import React, { useState } from "react";
import {useRouter} from "next/navigation";
import { SelectChangeEvent } from "@mui/material";
import {
  Box,
  Card,
  CardContent,
  Typography,
  Select,
  MenuItem,
} from "@mui/material";

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
  const [value, setValue] = useState<string>();
  const router = useRouter();
 
  
  const handleSelectChange = (event: SelectChangeEvent<string>) => {
    const value = event.target.value as string;
    setValue(value);

    if (value === "newworker") {
      router.push(`/components/${id}/overview`);
    } else if (value === "settings") {
      router.push(`/components/${id}/settings`);
    }
  };
  return (
    <Card
      sx={{
        borderRadius: 2,
        minWidth: "200px",
        "&:hover": { cursor: "pointer", boxShadow: "0px 5px 10px 0px #666"
        },
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

          <Select value={value} variant="standard" 
           onChange={handleSelectChange}
           onClick={(e) => e.stopPropagation()}
           >
            <MenuItem value="newworker">New Worker</MenuItem>
            <MenuItem value="settings">Settings</MenuItem>
          </Select>
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
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Running
            </Typography>
            <Typography variant="body2" sx={{ color: "#AAAAAA" }}>
              0 ▶
            </Typography>
          </Box>
          <Box>
            <Typography
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Idle
            </Typography>
            <Typography variant="body2" sx={{ color: "#AAAAAA" }}>
              0 ⏸
            </Typography>
          </Box>
          <Box>
            <Typography
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Suspended
            </Typography>
            <Typography variant="body2" sx={{ color: "#AAAAAA" }}>
              0 ⏹
            </Typography>
          </Box>
          <Box>
            <Typography
              variant="subtitle2"
              sx={{ fontWeight: 600, marginBottom: 0.5 }}
            >
              Failed
            </Typography>
            <Typography variant="body2" sx={{ color: "#AAAAAA" }}>
              0 ⚠
            </Typography>
          </Box>
        </Box>

        <Box sx={{ display: "flex", gap: 1, alignItems: "center" }}>
          <Typography className=" bg-[#787676] text-white px-2 py-1 rounded-md text-sm">
            v{version}
          </Typography>
          <Typography
            variant="body2"
            className="border border-[#555] px-2 rounded-md"
          >
            {`${exports} Exports`}
          </Typography>
          <Typography
            variant="body2"
            className="border border-[#555] px-2 rounded-md"
          >
            {`${size} MB`}
          </Typography>
          <Typography
            variant="body2"
            className="border border-[#555] px-2 rounded-md"
          >
            {componentType}
          </Typography>

          <Typography variant="body2" className="ml-5">
            {time}
          </Typography>
        </Box>
      </CardContent>
    </Card>
  );
};

export default ComponentInfoCard;
