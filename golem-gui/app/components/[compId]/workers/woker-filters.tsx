"use client";

import React, { useState } from "react";
import {
  Box,
  FormControl,
  InputLabel,
  Select,
  MenuItem,
  TextField,
  ListSubheader,
  Stack,
} from "@mui/material";
import { useParams, useRouter, useSearchParams } from "next/navigation";
import useSWR from "swr";

const statuses = [
  "Running",
  "Idle",
  "Suspended",
  "Interrupted",
  "Retrying",
  "Failed",
  "Exited",
];

const WorkerFilters = ({ compId }: { compId: string }) => {
  const router = useRouter();
  const searchParams = useSearchParams();
  const [workerStatus, setWorkerStatus] = useState<string[]>([]);
  const [version, setVersion] = useState("");
  const [searchQuery, setSearchQuery] = useState("");

  const {data, isLoading, error} = useSWR(`v1/components/${compId}`)

  const handleStatusChange = (selectedStatuses: string[]) => {
    setWorkerStatus(selectedStatuses);
    const params = new URLSearchParams(searchParams);
    if (selectedStatuses.length === 0) {
      params.delete("workerStatus");
    } else {
      params.set("workerStatus", JSON.stringify(selectedStatuses));
    }
    router.push(`/components/${compId}/workers?${params.toString()}`);
  };



  const handleVersionChange = (version: string) => {
    setVersion(version);
    const params = new URLSearchParams(searchParams);
    if (version) {
      params.set(
        "workerVersion",
        JSON.stringify({ version: Number(version), comparator: "Equal" })
      );
    } else {
      params.delete("workerVersion");
    }
    router.push(`/components/${compId}/workers?${params.toString()}`);
  };

  return (
    <Stack direction="row" spacing={2} mb={3}>
      {/* Worker Status Filter */}
      <FormControl variant="outlined" size="medium" sx={{ minWidth: 150 }}>
        <InputLabel>Worker Status</InputLabel>
        <Select
          multiple
          value={workerStatus}
          onChange={(e) =>
            handleStatusChange(
              Array.isArray(e.target.value) ? e.target.value : [e.target.value]
            )
          }
          renderValue={(selected) => selected.join(", ")}
          MenuProps={{
            PaperProps: {
              sx: { maxHeight: 300 },
            },
          }}
          displayEmpty
        >
          <ListSubheader>
            <TextField
              placeholder="Search..."
              variant="standard"
              fullWidth
              InputProps={{
                disableUnderline: true,
                startAdornment: <Box sx={{ marginRight: 1 }}>🔍</Box>,
              }}
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
            />
          </ListSubheader>
          {statuses
            .filter((status) =>
              status.toLowerCase().includes(searchQuery.toLowerCase())
            )
            .map((status) => (
              <MenuItem key={status} value={status}>
                <Box
                  component="span"
                  sx={{ display: "flex", alignItems: "center" }}
                >
                  <input
                    type="checkbox"
                    checked={workerStatus.includes(status)}
                    readOnly
                    style={{ marginRight: 8 }}
                  />
                  {status}
                </Box>
              </MenuItem>
            ))}
        </Select>
      </FormControl>

      {/* Version Filter */}
      <FormControl variant="outlined" size="medium" sx={{ minWidth: 150 }}>
        <InputLabel>Version</InputLabel>
        <Select
          value={version}
          onChange={(e) => handleVersionChange(e.target.value)}
          MenuProps={{
            PaperProps: { sx: { maxHeight: 300 } },
          }}
          displayEmpty
        >
          {[0, 1, 2].map((v) => (
            <MenuItem key={v} value={v}>
              {v}
            </MenuItem>
          ))}
        </Select>
      </FormControl>
    </Stack>
  );
};

export default WorkerFilters;
