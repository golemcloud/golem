import { useMemo } from "react";
import {
  Grid2 as Grid,
  Paper,
  Typography,
  Divider,
  List,
  ListItem,
  ListItemText,
  Box,
} from "@mui/material";
import useComponents from "@lib/hooks/use-component";
import { useSearchParams } from "react-router-dom";
import { ComponentExport, WorkerFunction } from "@lib/types/api";
import SecondaryHeader from "@ui/secondary-header";
import ErrorBoundary from "@ui/error-boundary";
import { useCustomParam } from "@lib/hooks/use-custom-param";
import InvokePage from "@components/components/invoke";

const OverviewEphemeral = () => {
  const { compId } = useCustomParam();
  const [params] = useSearchParams();
  const version = params?.get("version");

  const { components, isLoading, error } = useComponents(
    compId,
    version ?? "latest"
  );
  const [latestComponent] = components;

  const exports = useMemo(() => {
    const metaExports = (latestComponent?.metadata?.exports ||
      []) as ComponentExport[];
    return metaExports.flatMap((expo: ComponentExport) =>
      "functions" in expo
        ? expo.functions?.map(
            (fun: WorkerFunction) => `${expo.name}.${fun.name}`
          )
        : expo.name
    );
  }, [latestComponent?.metadata?.exports]);

  return (
    <>
      <SecondaryHeader variant="components" hideNew={true} />
      {error && <ErrorBoundary message={error} />}
      <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
        <div className="mx-auto max-w-2xl lg:max-w-none py-4">
          {!isLoading && (
            <Grid container spacing={1}>
              {/* Exports Section */}
              <Grid size={{ xs: 12,lg:4 }}>
                <Box className=" px-2 md:px-6 lg:px-8 xl:px-0 mt-3">
                <Paper
                  sx={{ bgcolor: "#1E1E1E", minHeight: 550 }}
                  className="border"
                >
                  <Typography variant="h6" className="p-5">
                    Exports
                  </Typography>
                  <Divider className="my-1 bg-border" />
                  <List className="px-7">
                    {exports.slice(0, 13).map((item, index) => (
                      <ListItem key={index} divider className="border-border">
                        <ListItemText primary={item} />
                      </ListItem>
                    ))}
                  </List>
                </Paper>
                </Box>
              </Grid>

              {/* Worker Status */}
              <Grid size={{ xs: 12,lg:8 }}>
                {exports.length > 0 ? (
                  <InvokePage />
                ) : (
                  <Typography className="pl-5 pt-5">
                    No Invocations found
                  </Typography>
                )}
              </Grid>
            </Grid>
          )}
        </div>
      </div>
    </>
  );
};

export default OverviewEphemeral;
