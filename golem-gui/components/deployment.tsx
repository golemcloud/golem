"use client";
import {
  Box,
  Typography,
  Paper,
  Stack,
  List,
} from "@mui/material";
import { Loader } from "lucide-react";
import { ApiDeployment } from "@/types/api";
import AddIcon from "@mui/icons-material/Add";
import { useState } from "react";
import DeploymentCreationPage from "@/components/deployment-creation";
import useApiDeployments from "@/lib/hooks/use-api-deployments";
import CustomModal from "./CustomModal";
import { Button2 as Button, Button2 } from "./ui/button";
import ErrorBoundary from "./erro-boundary";
import useApiDefinitions from '@/lib/hooks/use-api-definitons';

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
  const { apiDeployments, addApiDeployment, isLoading} =
    useApiDeployments(apiId);

const {getApiDefintion, isLoading:apiLoading, error: requestError} = useApiDefinitions(apiId)
const {error} = (!apiLoading && getApiDefintion() || {});

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
          className="border"
          elevation={3}
          sx={{
            p: 3,
            mb: 3,
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
                variant="primary"
                className="rounded-lg"
                startIcon={<AddIcon />}
                sx={{
                  textTransform: "none",
                }}
                onClick={handleOpen}
              >
                New
              </Button>
            </Stack>

            {isLoading && <Loader className="self-center" />}
          </Stack>
          {(error || requestError)&& (
           <ErrorBoundary message={requestError || error}/>
          )}
          {!isLoading && !error && deployments.length === 0 ? (
            <Typography variant="body2" className="text-muted-foreground">
              No Deployments for this API version.
            </Typography>
          ) : (
            //TODO: Add pagination List
            <List className="space-y-4 p-2">
              {Object.values(depolymentMap)?.map(
                (deployments: ApiDeployment[], dIndex: number) => {
                  const deployment = deployments[0];
                  return (
                    <Box
                      key={`${deployment.createdAt}_${dIndex}`}
                      className="px-4 py-6 flex justify-between border rounded-lg dark:hover:bg-[#373737] hover:bg-[#C0C0C0] cursor-pointer"

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
                      <Button2
                       variant="success"
                      >
                        Active
                      </Button2>
                    </Box>
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
