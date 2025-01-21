"use client";

import React, { useState } from "react";
import {
  Box,
  Typography,
  Grid2 as Grid,
  Paper,
  Divider,
  Stack,
} from "@mui/material";
import { Button2 as Button } from "@/components/ui/button";
import { Pencil, Trash } from "lucide-react";
import { ApiRoute } from "@/types/api";
import TryItOut from "./try-it-out";
import NewRouteForm from "./new-route";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { useCustomParam } from "@/lib/hooks/use-custom-param";
import { AlertDialogDemo } from "./confirmation-dialog";
import { useRouter } from "next/navigation";
import Link from "next/link";
import { TryOutlined } from "@mui/icons-material";

const ApiDetails = ({
  route,
  version,
  noRedirect,
}: {
  route: ApiRoute;
  version: string;
  noRedirect?: boolean;
}) => {
  const { apiId } = useCustomParam();

  const router = useRouter();
  const { deleteRoute } = useApiDefinitions(apiId);
  const handleDelete = async (
    e: React.MouseEvent<HTMLButtonElement>
  ): Promise<void> => {
    console.log("delete route");
    e.preventDefault();
    try {
      await deleteRoute(route!, version);
      if (!noRedirect) {
        router.push(`/apis/${apiId}/overview?version=${version}`);
      }
    } catch (error) {
      throw error;
    }
  };
  const [open, setOpen] = useState<string | null>("view");

  const handleOpen = (
    e: React.MouseEvent<HTMLButtonElement, MouseEvent>,
    tab: string
  ) => {
    e.preventDefault();
    setOpen(tab);
  };

  return (
    <>
      <Box>
        <Box className="flex justify-between">
          <Box>
            <Typography variant="h5">{route?.path}</Typography>
            <Button variant="primary" size="icon_sm">
              {route?.method}
            </Button>
          </Box>
          {/* TODO: Use tab instead of buttons */}
          <Box sx={{ display: "flex", gap: 1 }}>
            <Button
              variant="primary"
              size="sm"
              endIcon={<TryOutlined />}
              onClick={(e) => handleOpen(e, "try_it_out")}
            >
              Try it out
            </Button>
            <Button
              variant="primary"
              size="sm"
              endIcon={<Pencil size={64} />}
              onClick={(e) => handleOpen(e, "update")}
            >
              Edit
            </Button>
            <Button
              variant="primary"
              size="sm"
              endIcon={<Pencil size={64} />}
              onClick={(e) => handleOpen(e, "view")}
            >
              View
            </Button>
            <AlertDialogDemo
              onSubmit={(e: React.MouseEvent<HTMLButtonElement>) =>
                handleDelete(e)
              }
              paragraph={
                "This action cannot be undone. This will permanently delete this route."
              }
              child={
                <Button variant="error" size="sm" endIcon={<Trash />}>
                  {" "}
                  Delete{" "}
                </Button>
              }
            />
          </Box>
        </Box>

        {/* Sections */}
        <Grid container spacing={2}>
          {/* Component */}
          <Grid size={12}>
            <Divider className="bg-border my-2" />
          </Grid>
          <Grid size={{ xs: 12, sm: 3 }} alignItems="center">
            <Typography variant="body2" className="text-muted-foreground">
              Component
            </Typography>
          </Grid>
          <Link
            href={`/components/${route?.binding?.componentId?.componentId}/overview?version=${route?.binding?.componentId?.version}`}
          >
            <Grid size={{ xs: 12, sm: 9 }} alignItems="center">
              <Typography variant="body2" fontFamily="monospace">
                {route?.binding?.componentId?.componentId}
                {"/"}
                {route?.binding?.componentId?.version}
              </Typography>
            </Grid>
          </Link>

          <Grid size={12}>
            <Divider className="bg-border my-2" />
          </Grid>

          {/*TODO: Path Parameters */}
          {route && open == "update" && (
            <NewRouteForm
              apiId={apiId}
              version={version}
              defaultRoute={route}
              onSuccess={() => setOpen("view")}
              noRedirect={noRedirect}
            />
          )}
          {route && open == "try_it_out" && (
            <TryItOut route={route} version={version} />
          )}
          {route && open == "view" && (
            <>
              <>
                <Grid size={{ xs: 12, sm: 3 }}>
                  <Typography variant="body2" className="text-muted-foreground">
                    Path Parameters
                  </Typography>
                </Grid>

                <Grid size={{ xs: 12, sm: 9 }}>
                  <Stack direction="row" gap={5} alignItems="center">
                    <Typography className="text-muted-foreground">
                      test{" "}
                    </Typography>
                    <Paper
                      elevation={0}
                      className="w-full"
                      sx={{
                        p: 2,
                        fontFamily: "monospace",
                        fontSize: "0.875rem",
                      }}
                    >
                      str
                    </Paper>
                  </Stack>
                </Grid>
                <Grid size={12}>
                  <Divider className="bg-border my-2" />
                </Grid>
              </>
              {/*TODO: Request Body */}
              <Grid size={{ xs: 12, sm: 3 }}>
                <Typography variant="body2" className="text-muted-foreground">
                  Request Body
                </Typography>
              </Grid>
              <Grid size={{ xs: 12, sm: 9 }}>
                <Paper
                  elevation={0}
                  sx={{ p: 2, fontFamily: "monospace", fontSize: "0.875rem" }}
                >
                  Value will come from the request body
                </Paper>
              </Grid>

              <Grid size={12}>
                <Divider className="bg-border my-2" />
              </Grid>

              <Grid size={{ xs: 12, sm: 3 }}>
                <Typography variant="body2">
                  <Box display="flex" flexDirection="column" gap={1}>
                    <span className="text-muted-foreground">Response</span>
                    <Button
                      variant="primary"
                      size="icon_sm"
                      className="font-mono w-fit"
                    >
                      Rib
                    </Button>
                  </Box>
                </Typography>
              </Grid>

              <Grid size={{ xs: 12, sm: 9 }}>
                <Paper
                  elevation={0}
                  sx={{ p: 2, fontFamily: "monospace", fontSize: "0.875rem" }}
                >
                  {route?.binding?.response}
                </Paper>
              </Grid>

              <Grid size={12}>
                <Divider className="bg-border my-2" />
              </Grid>

              {/* Worker Name */}
              <Grid size={{ xs: 12, sm: 3 }}>
                <Typography variant="body2">
                  <Box display="flex" flexDirection="column" gap={1}>
                    <span className="text-muted-foreground">Worker Name</span>
                    <Button
                      variant="primary"
                      size="icon_sm"
                      className="font-mono w-fit"
                    >
                      Rib
                    </Button>
                  </Box>
                </Typography>
              </Grid>
              <Grid size={{ xs: 12, sm: 9 }}>
                <Paper
                  elevation={0}
                  sx={{ p: 2, fontFamily: "monospace", fontSize: "0.875rem" }}
                >
                  {route?.binding?.workerName}
                  <br />
                </Paper>
              </Grid>
            </>
          )}
        </Grid>
      </Box>
    </>
  );
};

export default ApiDetails;
