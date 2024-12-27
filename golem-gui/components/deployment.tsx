"use client";
import {
  Box,
  Typography,
  Paper,
  Stack,
  List,
  Button,
  Alert,
} from "@mui/material";
import { Loader } from "lucide-react";
import { ApiDeployment } from "@/types/api";
import { Card } from "@/components/ui/card";
import AddIcon from "@mui/icons-material/Add";
import { useState } from "react";
import DeploymentCreationPage from "@/components/deployment-creation";
import useApiDeployments from "@/lib/hooks/use-api-deployments";
import CustomModal from "./CustomModal";

export default function DeploymentPage({
  apiId,
  limit,
}: {
  apiId: string;
  limit?: number;
}) {
  //TODO to move this do separate custom hook so that we can resuse.
  const [open, setOpen] = useState(false);
  const handleOpen = () => setOpen(true);
  const handleClose = () => setOpen(false);
  const { apiDeployments, addApiDeployment, isLoading, error } =
    useApiDeployments(apiId);
  const deployments = limit ? apiDeployments.slice(0, limit) : apiDeployments;
  const depolymentMap = deployments?.reduce<Record<string, ApiDeployment[]>>(
    (obj, deployment: ApiDeployment) => {
      const key = `${deployment.site.host}__${deployment.site.subdomain}`;
      if (key in obj) {
        obj[key].push(deployment);
      } else {
        obj[key] = [deployment];
      }
      return obj;
    },
    {}
  );

  return (
    <>
      <Box>
        {/* Active Deployments Section */}
        <Paper
          className="bg-[#333]"
          elevation={3}
          sx={{
            p: 3,
            mb: 3,
            color: "text.primary",
            backgroundColor: "#333", // Use this for the background color
            border: 1,
            borderColor: "divider",
            borderRadius: 2,
          }}
        >
          <Stack
            sx={{
              mb: 2,
            }}
          >
            <Stack
              direction={"row"}
              justifyContent={"space-between"}
              alignItems={"center"}
            >
              <Typography variant="h6">Active Deployments</Typography>
              <Button
                variant="outlined"
                startIcon={<AddIcon />}
                sx={{
                  textTransform: "none",
                  marginLeft: "2px",
                }}
                onClick={handleOpen}
              >
                New
              </Button>
            </Stack>

            {isLoading && <Loader className="self-center" />}
          </Stack>
          {error && (
            <Box sx={{ display: "flex", justifyContent: "center" }}>
              <Alert severity="error">{error}</Alert>
            </Box>
          )}
          {!isLoading && !error && deployments.length === 0 ? (
            <Typography variant="body2">
              No Deployments for this API version.
            </Typography>
          ) : (
            //TODO: Add pagination List
            <List className="space-y-4 p-2">
              {Object.values(depolymentMap)?.map(
                (deployments: ApiDeployment[], dIndex: number) => {
                  const deployment = deployments[0];
                  return (
                    <Card
                      key={`${deployment.createdAt}_${dIndex}`}
                      className="px-4 py-6 flex hover:"
                    >
                      <Stack>
                        <Typography gutterBottom className="font-bold">
                          {deployment.site.subdomain}
                          {"."}
                          {deployment.site.host}
                        </Typography>
                        <Typography gutterBottom className="font-bold">
                          {deployment.site.subdomain}
                        </Typography>
                      </Stack>
                      <Typography
                        border={1}
                        borderRadius={2}
                        className={
                          "px-4 py-1 text-sm ml-auto self-center hover:"
                        }
                      >
                        Active
                      </Typography>
                    </Card>
                  );
                }
              )}
            </List>
          )}
        </Paper>
      </Box>
      <CustomModal
        open={open}
        onClose={handleClose}
        heading={"Create deployment"}
      >
        <DeploymentCreationPage
          addDeployment={addApiDeployment}
          apiId={apiId}
          onSuccess={handleClose}
        />
      </CustomModal>
    </>
  );
}
