"use client";
import { Box, Typography, Stack, List } from "@mui/material";
import { Loader } from "lucide-react";
import { Card } from "@/components/ui/card";
import { ApiDefinition, ApiRoute } from "../types/api";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { useState } from "react";
import CustomModal from "@/components/CustomModal";
import NewRouteForm from "./new-route";

export function RouteList({
  apiDefintion,
  isLoading,
  limit,
}: {
  apiDefintion?: ApiDefinition;
  isLoading: boolean;
  limit?: number;
}) {
  let routes = (apiDefintion?.routes || []) as ApiRoute[];
  routes = limit ? routes.slice(0, limit) : routes;
  const [route, setRoute] = useState<ApiRoute | null>(null);

  return (
    <>
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
                    className="px-4 py-6 flex border"
                    onClick={(e) => {
                      e.preventDefault();
                      setRoute(route);
                    }}
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
      <>
        <CustomModal open={!!route} onClose={() => setRoute(null)}>
          {apiDefintion && (
            <NewRouteForm
              apiId={apiDefintion?.id}
              version={apiDefintion?.version}
              defaultRoute={route}
              onSuccess={() => setRoute(null)}
            />
          )}
        </CustomModal>
      </>
    </>
  );
}

export default function RoutePage({
  apiId,
  version,
  limit,
}: {
  apiId: string;
  version?: string | null;
  limit?: number;
}) {
  //TODO to move this do separate custom hook so that we can resuse.
  const { isLoading, getApiDefintion } = useApiDefinitions(apiId, version);
  const { data: apiDefintion } = getApiDefintion();

  return (
    <RouteList
      isLoading={isLoading}
      apiDefintion={apiDefintion}
      limit={limit}
    />
  );
}
