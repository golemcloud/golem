"use client";

import React, { useState } from "react";
import {
  Box,
  Typography,
  Stack,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import { useWorkerFind } from "@/lib/hooks/use-worker";
import { useParams, useRouter } from "next/navigation";
import { Crosshair, Loader } from "lucide-react";
import { Worker } from "@/types/api";
import CreateWorker from "@/components/create-worker";
import CustomModal from "@/components/CustomModal";
import SecondaryHeader from "@/components/ui/secondary-header";
import { Button2 } from "@/components/ui/button";
import WorkerInfoCard from "@/components/worker-info-card";
import {StatusFilter, VersionFilter, Search, CustomDatePickFilter} from "./workers-filter";
import ErrorBoundary from "@/components/erro-boundary";
import { useCustomParam } from "@/lib/hooks/use-custom-param";


const WorkerListWithDropdowns = () => {
  const router = useRouter();
  //TO DO: let show filters in url so that user can share the url to others.
  // const { compId } = useParams<{ compId: string }>();
  const { compId } = useCustomParam();
  const [open, setOpen] = useState(false);

  const handleClose = () => setOpen(false);

  //need to integrate pagination or scroll on lcomponentIdoad needs to implemented or addd show more at the end on click we need to next set of data
  const { data, isLoading, error, triggerNext } = useWorkerFind(compId, 10, true);
  const workers = !isLoading && !error ? data : []

  return (
    <>
      <Box sx={{ display: { xs: "block", md: "none" } }}>
        <SecondaryHeader onClick={() => {}} variant="components" />
      </Box>
      {error && <ErrorBoundary message={error}/>}
      <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
        <div className="mx-auto max-w-2xl lg:max-w-none py-4">
          {/* Search Box */}
          <Box
            display="flex"
            justifyContent="space-between"
            alignItems="center"
            mb={3}
            gap={2}
          >
            <Search />
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
          <Stack direction="row" className="flex gap-2 justify-center" mb={3} sx={{ flexWrap: 'wrap' }}>
              <div className="w-[220px]"><StatusFilter /></div>
              <div className="w-[220px]"><VersionFilter /></div>
              <CustomDatePickFilter label="Created After" searchKey={"workerAfter"}/>
              <CustomDatePickFilter label="Created Before" searchKey={"workerBefore"}/>
          </Stack>

          {/* No Workers Found */}
          {!isLoading && workers?.length == 0 && (
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
              <Button2
                variant="dropdown"
                size="lg"
              >
                Retry
              </Button2>
            </Box>
          )}

              <Box>
                {isLoading ? (
                  <Loader />
                ) : (
                  <Stack gap={4}>
                    {workers?.map((worker: Worker) => {
                      return (
                      <WorkerInfoCard
                        key={worker.workerId.workerName}
                        worker={worker}
                        onClick={() =>
                          router.push(`/components/${compId}/workers/${worker.workerId.workerName}`)
                        }
                      />
                      );
                    })}
                  </Stack>
                )}
              </Box>
              {/*TODO: for now cursor is handled like this. but this can be improved on scroll load and some other things */}
              {triggerNext && <Stack alignItems={"center"} sx={{pt:2}}>
                <Button2 onClick={triggerNext}>more..</Button2>
                </Stack>}
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
