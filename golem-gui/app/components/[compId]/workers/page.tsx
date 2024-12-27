"use client";

import React, { useState } from "react";
import {
  Box,
  TextField,
  Button,
  Typography,
  Stack,
  IconButton,
  Select,
  MenuItem,
  FormControl,
  ListSubheader,
  Card,
  InputLabel,
} from "@mui/material";
import RefreshIcon from "@mui/icons-material/Refresh";
import AddIcon from "@mui/icons-material/Add";
import { LocalizationProvider } from "@mui/x-date-pickers/LocalizationProvider";
import { AdapterDateFns } from "@mui/x-date-pickers/AdapterDateFnsV3";
import { DatePicker } from "@mui/x-date-pickers/DatePicker";
import useWorkers from "@/lib/hooks/use-worker";
import { useParams, useRouter } from "next/navigation";
import { Loader } from "lucide-react";
import { Worker } from "@/types/api";
import CreateWorker from "@/components/create-worker";
import CustomModal from "@/components/CustomModal";

const WorkerListWithDropdowns = () => {
  const [workerStatus, setWorkerStatus] = useState<string[]>([]);
  const router = useRouter();
  //TO DO: let show filters in url so that user can share the url to others.
  const { compId } = useParams<{ compId: string }>();
  const [version, setVersion] = useState("");
  const [createdAfter, setCreatedAfter] = useState<Date | null>(null);
  const [createdBefore, setCreatedBefore] = useState<Date | null>(null);
  const [open, setOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState(""); // For searching statuses

  const handleClose = ()=>setOpen(false)

  //need to integrate the filter logic here. and pagination or scroll on load needs to implemented or addd show more at the end on click we need to next set of data
  const { workers, isLoading } = useWorkers(compId);
  const statuses = [
    "Running",
    "Idle",
    "Suspended",
    "Interrupted",
    "Retrying",
    "Failed",
    "Exited",
  ];

  const filteredStatuses = statuses.filter((status) =>
    status.toLowerCase().includes(searchQuery.toLowerCase())
  );


  return (
    <>
    <LocalizationProvider dateAdapter={AdapterDateFns}>
      <Box
        sx={{
          marginBottom: 3,
          padding: 3,
          display: "flex",
          flexDirection: "column",
        }}
      >
        {/* Search Box */}
        <Stack direction="row" spacing={2} mb={3}>
          <TextField
            placeholder="Worker name..."
            variant="outlined"
            fullWidth
            InputProps={{
              startAdornment: (
                <Typography sx={{ marginRight: 1 }}>🔍</Typography>
              ),
            }}
          />
          <IconButton sx={{ color: "white" }}>
            <RefreshIcon />
          </IconButton>
          <Button
            variant="contained"
            startIcon={<AddIcon />}
            sx={{
              backgroundColor: "#2962FF",
              "&:hover": { backgroundColor: "#0039CB" },
            }}
            onClick={(e)=>{e.preventDefault();setOpen(true)}}
          >
            New
          </Button>
        </Stack>

        <Stack direction="row" spacing={2} mb={3}>
         
          <FormControl variant="outlined" size="medium" sx={{ minWidth: 150 }}>
            <InputLabel>Worker Status</InputLabel>
            <Select
              multiple
              value={workerStatus}
              onChange={(e) => setWorkerStatus(e.target.value)}
              renderValue={(selected) => selected.join(", ")} 
              MenuProps={{
                PaperProps: {
                  sx: {
                    maxHeight: 300,
                  },
                },
              }}
              displayEmpty
            >
              {/* Separate search input */}
              <ListSubheader>
                <TextField
                  placeholder="Search..."
                  variant="standard"
                  fullWidth
                  InputProps={{
                    disableUnderline: true,
                    startAdornment: (
                      <Typography sx={{ marginRight: 1 }}>🔍</Typography>
                    ),
                  }}
                  value={searchQuery} 
                  onChange={(e) => setSearchQuery(e.target.value)} 
                  sx={{
                    padding: 1,
                    borderRadius: 1,
                    border: "1px solid gray",
                  }}
                />
              </ListSubheader>
              {/* Filtered statuses */}
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
              {/* No results found */}
              {statuses.filter((status) =>
                status.toLowerCase().includes(searchQuery.toLowerCase())
              ).length === 0 && <MenuItem disabled>No results found</MenuItem>}
            </Select>
          </FormControl>

          {/* Version */}
          <FormControl variant="outlined" size="medium" sx={{ minWidth: 150 }}>
            <Select
              value={version}
              onChange={(e) => setVersion(e.target.value)}
              MenuProps={{
                PaperProps: {
                  sx: {
                    maxHeight: 300,
                  },
                },
              }}
              displayEmpty
            >
              <ListSubheader>
                <TextField
                  variant="standard"
                  fullWidth
                  InputProps={{
                    disableUnderline: true,
                    startAdornment: (
                      <Typography sx={{ marginRight: 1 }}>🔍</Typography>
                    ),
                  }}
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  sx={{
                    padding: 1,
                    borderRadius: 1,
                    border: "1px solid gray",
                  }}
                />
              </ListSubheader>
              {["v1", "v2", "v3"]
                .filter((v) =>
                  v.toLowerCase().includes(searchQuery.toLowerCase())
                )
                .map((v) => (
                  <MenuItem key={v} value={v}>
                    {v}
                  </MenuItem>
                ))}
              {["v1", "v2", "v3"].filter((v) =>
                v.toLowerCase().includes(searchQuery.toLowerCase())
              ).length === 0 && <MenuItem disabled>No results found</MenuItem>}
            </Select>
          </FormControl>

          {/* Created After */}
          {/* <DatePicker
            label="Created After"
            value={createdAfter}
            onChange={(date) => setCreatedAfter(date)}
            renderInput={(params) => (
              <TextField
                {...params}
                sx={{
                  ".MuiOutlinedInput-notchedOutline": { borderColor: "gray" },
                }}
              />
            )}
          /> */}

          {/* Created Before */}
          {/* <DatePicker
            label="Created Before"
            value={createdBefore}
            onChange={(date) => setCreatedBefore(date)}
            renderInput={(params) => (
              <TextField
                {...params}
                sx={{
                  ".MuiOutlinedInput-notchedOutline": { borderColor: "gray" },
                }}
              />
            )}
          /> */}
        </Stack>

        {/* No Workers Found */}
        {!isLoading && workers.length == 0 && (
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
              No workers matched the current search
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
          {isLoading ? (
            <Loader />
          ) : (
            <Stack gap={4}>
              {workers?.map((worker: Worker) => {
                return (
                  <Card key={worker?.workerId.workerName} className="p-4" onClick={()=>router.push(`/components/${compId}/workers/${worker.workerId.workerName}`)}>
                    <Stack gap={2}>
                      <Typography>{worker?.workerId.workerName}</Typography>
                      <Stack
                        direction="row"
                        justifyContent={"space-between"}
                        alignItems={"center"}
                      >
                        <Stack>
                          <Typography>Status</Typography>
                          <Typography>{worker.status}</Typography>
                        </Stack>
                        <Stack>
                          <Typography>Memory</Typography>
                          <Typography>
                            {worker.totalLinearMemorySize}
                          </Typography>
                        </Stack>
                        <Stack>
                          <Typography>Pending Invocation</Typography>
                          <Typography>
                            {worker.pendingInvocationCount}
                          </Typography>
                        </Stack>
                        <Stack>
                          <Typography>Resources</Typography>
                          <Typography>
                            {Object.values(worker.ownedResources).length}
                          </Typography>
                        </Stack>
                      </Stack>
                    </Stack>
                    <Stack direction="row" gap={4} marginTop={2}>
                      <Typography className="border p-1 px-4">V{worker.componentVersion}</Typography>
                      <Typography className="border p-1 px-4">Env{" "}{Object.values(worker.env).length}</Typography>
                      <Typography className="border p-1 px-4">Agrs{" "}{worker.args.length}</Typography>
                    </Stack>
                  </Card>
                );
              })}
            </Stack>
          )}
        </Box>
      </Box>
    </LocalizationProvider>
    <CustomModal open={open} onClose={handleClose} heading={"Create new Worker"}>
          <CreateWorker compId={compId} onSuccess={handleClose}/>
    </CustomModal>
    </>
  );
};

export default WorkerListWithDropdowns;
