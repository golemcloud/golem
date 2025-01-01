"use client";

import React, { useState } from "react";
import {
  Box,
  TextField,
  Button,
  Typography,
  Stack,
  Card,
  InputAdornment,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import useWorkers from "@/lib/hooks/use-worker";
import { useParams, useRouter } from "next/navigation";
import { Crosshair, Loader } from "lucide-react";
import { Worker } from "@/types/api";
import CreateWorker from "@/components/create-worker";
import CustomModal from "@/components/CustomModal";
import SecondaryHeader from "@/components/ui/secondary-header";
import SearchIcon from "@mui/icons-material/Search";
import { Button2 } from "@/components/ui/button";
import DropDown from "./drop-down";

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

  const handleClose = () => setOpen(false);

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
      <Box sx={{ display: { xs: "block", md: "none" } }}>
        <SecondaryHeader onClick={() => {}} variant="components" />
      </Box>
      <div className="mx-auto max-w-7xl px-6 lg:px-8">
        <div className="mx-auto max-w-2xl lg:max-w-none py-4">
          {/* Search Box */}
          <Box
            display="flex"
            justifyContent="space-between"
            alignItems="center"
            mb={3}
            gap={2}
          >
            <TextField
              placeholder="Worker Name..."
              variant="outlined"
              className="flex-1"
              value={searchQuery}
              size="small"
              onChange={(e) => setSearchQuery(e.target.value)}
              InputProps={{
                startAdornment: (
                  <InputAdornment position="start">
                    <SearchIcon sx={{ color: "grey.500" }} />
                  </InputAdornment>
                ),
              }}
            />

            <Box className="border p-2 text-lg rounded-md cursor-pointer">
              <Crosshair size="22px" />
            </Box>

            <Button2
              variant="primary"
              size="md"
              endIcon={<AddIcon />}
              onClick={(e) => {
                e.preventDefault();
                setOpen(true);
              }}
            >
              New
            </Button2>
          </Box>

          {/* Filters */}
          <Stack direction="row" gap={2} mb={3}>
            <DropDown />
            {/* <DropDown />
            <DropDown />
            <DropDown /> */}
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
                    <Card
                      key={worker?.workerId.workerName}
                      className="p-4"
                      onClick={() =>
                        router.push(
                          `/components/${compId}/workers/${worker.workerId.workerName}`
                        )
                      }
                    >
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
                        <Typography className="border p-1 px-4">
                          V{worker.componentVersion}
                        </Typography>
                        <Typography className="border p-1 px-4">
                          Env {Object.values(worker.env).length}
                        </Typography>
                        <Typography className="border p-1 px-4">
                          Agrs {worker.args.length}
                        </Typography>
                      </Stack>
                    </Card>
                  );
                })}
              </Stack>
            )}
          </Box>

          <CustomModal
            open={open}
            onClose={handleClose}
            heading={"Create new Worker"}
          >
            <CreateWorker compId={compId} onSuccess={handleClose} />
          </CustomModal>
        </div>
      </div>
    </>
  );
};

export default WorkerListWithDropdowns;
