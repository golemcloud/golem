"use client";
import { Box, Typography, Paper, Stack, List } from "@mui/material";
import useSWR from "swr";
import { Loader } from "lucide-react";
import { fetcher } from "@/lib/utils";
import { Deployment } from "@/types/api";
import { Card } from "@/components/ui/card";

export default function DeploymentPage({apiId, limit}:{apiId:string, limit?:number}) {
  //TODO to move this do separate custom hook so that we can resuse.
  const { data, isLoading } = useSWR(
    `?path=api/deployments?api-definition-id=${apiId!}`,
    fetcher
  );
  let deployments = (data?.data || []) as Deployment[];
  deployments = limit ? deployments.slice(0, limit) : deployments
  const depolymentMap = deployments?.reduce<Record<string, Deployment[]>>(
    (obj, deployment: Deployment) => {
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
          <Typography variant="h6">Active Deployments</Typography>

          {isLoading && <Loader className="self-center" />}
        </Stack>
        {!isLoading && deployments.length === 0 ? (
          <Typography variant="body2">
            No routes defined for this API version.
          </Typography>
        ) : (
          //TODO: Add pagination List
          <List className="space-y-4 p-2">
            {Object.values(depolymentMap)?.map(
              (deployments: Deployment[], dIndex: number) => {
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
    </Box>
  );
}
