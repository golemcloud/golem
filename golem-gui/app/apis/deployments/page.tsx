"use client";
import {
  Box,
  Typography,
  Paper,
  Divider,
  Stack,
  List,
} from "@mui/material";
import useSWR from "swr";
import { Code2Icon, Loader, MenuIcon } from "lucide-react";
import { fetcher } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@radix-ui/react-dropdown-menu";
import { Button2 as CustomButton } from "@/components/ui/button";
import { ApiDeployment as Deployment} from "@/types/api";
import { Card } from "@/components/ui/card";
import DeploymentCreationPage from "../../../../golem-gui-react/src/components/apis/deployment-creation";
import { useState } from "react";
import CustomModal from "@/components/custom/custom-modal";
import { useCustomParam } from "@/lib/hooks/use-custom-param";

function DeploymentApiVersionDropDown({
  deployments,
  dIndex,
}: {
  deployments: Deployment[];
  dIndex?: number
}) {
  return (
    <Stack className="flex flex-row justify-between lg:flex-col md:flex-col">
      <Code2Icon className="self-center text-gray-500" />
      <DropdownMenu>
        {deployments?.map((deployment, index: number) => {
          const api = deployment.apiDefinitions?.[0] || {}
          return (
            <div key={`${dIndex}__${index}`}>
              <DropdownMenuTrigger asChild>
                <CustomButton variant="outline">
                  <span>
                    {api.id}({api.version})
                  </span>
                </CustomButton>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onClick={() => {}}>
                  {api.id}({api.version})
                </DropdownMenuItem>
              </DropdownMenuContent>
            </div>
          );
        })}
      </DropdownMenu>
    </Stack>
  );
}

export default function Page() {
  const { apiId } = useCustomParam();
  const [open, setOpen] = useState<boolean>(false);
  
  //TODO to move this do separate custom hook so that we can resuse.
  const { data, isLoading } = useSWR(
    `?path=api/deployments`,
    fetcher
  );
  const deployments = (data?.data || []) as Deployment[];

  const depolymentMap = deployments?.reduce<Record<string, Deployment[]>>((obj, deployment:Deployment)=>{
    const key = `${deployment.site.host}__${deployment.site.subdomain}`
    if(key in obj){
      obj[key].push(deployment);
    } else {
      obj[key] = [deployment];
    } 
    return obj;
  }, {})

  const handleDelete = async (site:string) => {
    
    const response = await fetcher(
      `?path=api/deployments/${apiId}/${site}`,
      {
        method: "DELETE",
        headers: {
          "Content-Type": "application/json",
        },
      }
    );

    if (response.data === apiId) {
      console.log("successfully deleted");
      return;
    }
  };
  const handleClose = () => setOpen(false);
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
        <Box
          sx={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            mb: 2,
          }}
        >
          <Typography variant="h6">Active Deployments</Typography>
          {/* <Button
            variant="contained"
            color="primary"
            onClick={() => console.log("Button clicked")}
            disabled={isLoading || deployments?.length == 0}
          >
            View All
          </Button> */}
        </Box>
        {isLoading && <Loader />}
        {deployments.length === 0 ? (
          <Typography variant="body2" className="text-muted-foreground">
            No routes defined for this API version.
          </Typography>
        ) : (
          <List className="gap-y-8 p-2">
            {Object.values(depolymentMap)?.map((deployments: Deployment[], dIndex: number) => {
              const deployment = deployments[0];
              return (
                <Card
                  key={`${deployment.createdAt}_${dIndex}`}
                  className="px-4 py-6"
                >
                  <Stack
                    direction="row"
                    justifyContent={"space-between"}
                    alignItems={"center"}
                  >
                    <Typography gutterBottom className="font-bold">
                      {deployment.site.subdomain}
                      {"."}
                      {deployment.site.host}
                    </Typography>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <CustomButton variant="outline" size="icon">
                          <MenuIcon />
                          <span className="sr-only">Delete Deployment</span>
                        </CustomButton>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem onClick={()=>handleDelete(deployment.site.host)}>
                          Delete Deployment
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </Stack>
                  <Divider className="my-4" />
                  <Stack>
                    <Box className="flex justify-between">
                      <Typography
                        gutterBottom
                        className="self-center text-gray-500"
                      >
                        Subdomain
                      </Typography>
                      <Typography gutterBottom>
                        {deployment.site.subdomain || "unkown"}
                      </Typography>
                    </Box>
                    <Stack className="flex justify-between">
                      <Typography
                        gutterBottom
                        className="self-center text-gray-500"
                      >
                        Host
                      </Typography>
                      <Typography gutterBottom>
                        {deployment.site.host || "unkown"}
                      </Typography>
                    </Stack>
                    <DeploymentApiVersionDropDown
                      deployments={deployments}
                    />
                  </Stack>
                </Card>
              );
            })}
          </List>
        )}
      </Paper>
       {/* Modal for Creating New Deployment */}
       <CustomModal open={!!open} onClose={handleClose} heading="Create Deployment">
          <DeploymentCreationPage onSuccess={handleClose}/>
      </CustomModal>
    </Box>
  );
}

