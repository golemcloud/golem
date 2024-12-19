"use client";
import { Box, Typography, Stack, List } from "@mui/material";
import useSWR from "swr";
import { Loader } from "lucide-react";
import { fetcher } from "@/lib/utils";
import { Card } from "@/components/ui/card";
import { ApiDefinition, ApiRoute } from "../types/api";

export function RouteList({
  apiDefintion,
  isLoading,
  limit
}: {
  apiDefintion?: ApiDefinition;
  isLoading: boolean;
  limit?: number
}) {
  let routes = (apiDefintion?.routes || []) as ApiRoute[];
  routes = limit ? routes.slice(0, limit) : routes
  return (
    <Box>
      {/* Active Deployments Section */}
      <Stack
        sx={{
          mb: 2,
        }}
      >
        <Typography variant="h6">Active Routes</Typography>

        {isLoading && <Loader className="self-center" />}
      </Stack>
      {!isLoading && routes.length === 0 ? (
        <Typography variant="body2">
          No routes defined for this API version.
        </Typography>
      ) : (
        //TODO: Add pagination List
        <List className="space-y-4 p-2">
          {apiDefintion &&
            routes?.map((route: ApiRoute) => {
              return (
                <Card
                  key={`${apiDefintion.id}_${apiDefintion.version}_${route.method}_${route.path}`}
                  className="px-4 py-6 flex hover:"
                >
                  <Typography gutterBottom className="font-bold">
                    {route.path}
                  </Typography>

                  <Typography
                    border={1}
                    borderRadius={2}
                    className={"px-4 py-1 text-sm ml-auto self-center"}
                  >
                    {route.method}
                  </Typography>
                </Card>
              );
            })}
        </List>
      )}
    </Box>
  );
}

export default function RoutePage({
  apiId,
  version,
  limit
}: {
  apiId: string;
  version?: string;
  limit?:number
}) {
  //TODO to move this do separate custom hook so that we can resuse.
  const { data, isLoading } = useSWR(
    `?path=api/definitions?api-definition-id=${apiId!}`,
    fetcher
  );
  const apiDefintions = (data?.data || []) as ApiDefinition[];
  const apiDefintion = version
    ? apiDefintions.find((api) => api.version === version)
    : apiDefintions[apiDefintions.length - 1];

  return <RouteList isLoading={isLoading} apiDefintion={apiDefintion} limit={limit}/>;
}
