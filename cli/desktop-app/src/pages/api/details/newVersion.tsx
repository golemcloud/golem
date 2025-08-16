import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Loader2, PlusCircle } from "lucide-react";
import { Input } from "@/components/ui/input";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import * as z from "zod";
import { API } from "@/service";
import ErrorBoundary from "@/components/errorBoundary";
import { HttpApiDefinition } from "@/types/golemManifest.ts";

const newVersionSchema = z.object({
  version: z
    .string()
    .min(1, "Version is required")
    .regex(
      /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/,
      "Version must follow semantic versioning (e.g., 1.0.0)",
    )
    .refine(
      value => {
        // Parse version components
        const [major, minor, patch] = value.split(".").map(Number);
        return (
          Number.isInteger(major) &&
          Number.isInteger(minor) &&
          Number.isInteger(patch) &&
          (major || 0) >= 0 &&
          (minor || 0) >= 0 &&
          (patch || 0) >= 0
        );
      },
      {
        message: "Invalid version format. Must be valid semver (e.g., 1.0.0)",
      },
    ),
});

type NewVersionFormValues = z.infer<typeof newVersionSchema>;

export default function APINewVersion() {
  const navigate = useNavigate();
  const { apiName, version, appId } = useParams();
  const [isLoading, setIsLoading] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [apiDetails, setApiDetails] = useState<HttpApiDefinition[]>([]);
  const [activeApiDetails, setActiveApiDetails] =
    useState<HttpApiDefinition | null>(null);
  const [fetchError, setFetchError] = useState<string | null>(null);

  const form = useForm<NewVersionFormValues>({
    resolver: zodResolver(newVersionSchema),
    defaultValues: {
      version: "",
    },
  });

  // Watch version changes to validate against existing versions
  const watchedVersion = form.watch("version");

  useEffect(() => {
    if (watchedVersion && apiDetails.length > 0) {
      const versionExists = apiDetails.some(
        api => api.version === watchedVersion,
      );
      if (versionExists) {
        form.setError("version", {
          type: "manual",
          message: "This version already exists",
        });
      } else {
        form.clearErrors("version");
      }
    }
  }, [watchedVersion, apiDetails, form]);

  // Fetch API details with retry logic
  useEffect(() => {
    const fetchApiDetails = async (retryCount = 0) => {
      if (!apiName) return;

      try {
        setIsLoading(true);
        setFetchError(null);
        const response = await API.apiService.getApi(appId!, apiName);
        setApiDetails(response);
        setActiveApiDetails(response[response.length - 1]!);
      } catch (error) {
        console.error("Failed to fetch API details:", error);
        setFetchError("Failed to load API details. Please try again.");

        // Retry logic (max 3 attempts)
        if (retryCount < 3) {
          setTimeout(
            () => fetchApiDetails(retryCount + 1),
            1000 * (retryCount + 1),
          );
        }
      } finally {
        setIsLoading(false);
      }
    };

    fetchApiDetails();
    form.setValue("version", autoIncrementVersion());
  }, [apiName, version]);

  const autoIncrementVersion = () => {
    const [major, minor, patch] = version!.split(".").map(Number);
    return `${major}.${minor}.${(patch || 0) + 1}`;
  };

  const onSubmit = async (values: NewVersionFormValues) => {
    if (!activeApiDetails) {
      form.setError("version", {
        type: "manual",
        message: "No active API version selected",
      });
      return;
    }

    // Final validation check
    const versionExists = apiDetails.some(
      api => api.version === values.version,
    );
    if (versionExists) {
      form.setError("version", {
        type: "manual",
        message: "This version already exists",
      });
      return;
    }

    try {
      setIsSubmitting(true);
      const payload = {
        ...activeApiDetails,
        version: values.version,
        draft: true,
        createdAt: new Date().toISOString(),
      };
      await API.apiService.createApiVersion(appId!, payload);
      // .then(() => {
      navigate(`/app/${appId}/apis/${apiName}/version/${values.version}`);
      // });
      // throw new Error("yes o");
    } catch (error) {
      console.error("Failed to create new version:", error);
      form.setError("version", {
        type: "manual",
        message: "Failed to create new version. Please try again.",
      });
    } finally {
      setIsSubmitting(false);
    }
  };

  // Show error state if API fetch fails
  if (fetchError) {
    return (
      <div className="p-6 max-w-3xl mx-auto">
        <div className="flex flex-col items-center justify-center space-y-4 p-8 border rounded-lg bg-destructive/10">
          <p className="text-destructive font-medium">{fetchError}</p>
          <Button variant="outline" onClick={() => window.location.reload()}>
            Retry
          </Button>
        </div>
      </div>
    );
  }

  return (
    <ErrorBoundary>
      <main className="max-w-4xl mx-auto p-14">
        {isLoading ? (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-6 w-6 animate-spin" />
            <span className="ml-2">Loading API details...</span>
          </div>
        ) : (
          <Form {...form}>
            <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-6">
              <FormField
                control={form.control}
                name="version"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel className="text-base font-medium">
                      New Version
                    </FormLabel>
                    <FormControl>
                      <Input
                        placeholder="New Version prefix (0.1.0)"
                        {...field}
                      />
                    </FormControl>
                    <FormDescription>
                      Creating a copy of version {version}. Version must follow
                      semantic versioning (e.g., 1.0.0).
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <div className="flex justify-end">
                <Button type="submit" disabled={isSubmitting}>
                  {isSubmitting ? (
                    <>
                      <Loader2 className="mr-2 h-5 w-5 animate-spin" />
                      Creating...
                    </>
                  ) : (
                    <>
                      <PlusCircle className="mr-2 h-5 w-5" />
                      New Version
                    </>
                  )}
                </Button>
              </div>
            </form>
          </Form>
        )}
      </main>
    </ErrorBoundary>
  );
}
