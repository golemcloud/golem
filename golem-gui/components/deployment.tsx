"use client";
import { Box, Typography, Paper, Stack, List , Button, Modal, Container} from "@mui/material";
import useSWR from "swr";
import { Loader } from "lucide-react";
import { fetcher } from "@/lib/utils";
import { ApiDeployment } from "@/types/api";
import { Card } from "@/components/ui/card";
import AddIcon from "@mui/icons-material/Add";
import { useState } from "react";
import DeploymentCreationPage from "@/components/deployment-creation";

export default function DeploymentPage({apiId, limit}:{apiId:string, limit?:number}) {
  //TODO to move this do separate custom hook so that we can resuse.
  const [open, setOpen] = useState(false);
  const handleOpen = ()=>setOpen(true)
  const handleClose = ()=>setOpen(false)
  const { data, isLoading } = useSWR(
    `?path=api/deployments?api-definition-id=${apiId!}`,
    fetcher
  );
  let deployments = (data?.data || []) as ApiDeployment[];
  deployments = limit ? deployments.slice(0, limit) : deployments
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

  // const handleDelete = async (site: string) => {
  //   const response = await fetcher(`?path=api/deployments/${apiId}/${site}`, {
  //     method: "DELETE",
  //     headers: {
  //       "Content-Type": "application/json",
  //     },
  //   });

  //   if (response.data === apiId) {
  //     console.log("successfully deleted");
  //     return;
  //   }
  // };

  return (
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
          <Stack direction={"row"} justifyContent={"space-between"} alignItems={"center"}>
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
        {!isLoading && deployments.length === 0 ? (
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
                      className={"px-4 py-1 text-sm ml-auto self-center hover:"}
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
      <Modal open={open} onClose={handleClose}>
              <Container className="p-2">
                  <Paper className={"m-auto w-[80%] md:max-w-[45%] lg:max-w-[45%] p-4"}>
            <DeploymentCreationPage onCreation={handleClose}/>
            </Paper>
          </Container>
        </Modal>
    </Box>
  );
}
