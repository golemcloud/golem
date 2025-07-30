import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { PlusCircle, ArrowLeft, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Form,
  FormControl,
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

const createApiSchema = z.object({
  apiName: z
    .string()
    .min(3, "API Name must be at least 3 characters")
    .regex(
      /^[a-zA-Z][a-zA-Z_-]*$/,
      "API name must contain only letters and underscores",
    ),
  version: z
    .string()
    .min(1, "Version is required")
    .regex(
      /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/,
      "Version must follow semantic versioning (e.g., 1.0.0)",
    ),
});

type CreateApiFormValues = z.infer<typeof createApiSchema>;

const CreateAPI = () => {
  const navigate = useNavigate();
  const [isSubmitting, setIsSubmitting] = useState(false);
  const { appId } = useParams<{ appId: string }>();

  const form = useForm<CreateApiFormValues>({
    resolver: zodResolver(createApiSchema),
    defaultValues: {
      apiName: "",
      version: "0.1.0",
    },
  });

  const onSubmit = async (values: CreateApiFormValues) => {
    try {
      setIsSubmitting(true);
      await API.apiService.createApi(appId!, {
        id: values.apiName,
        version: values.version,
        routes: [],
      });
      navigate(
        `/app/${appId}/apis/${values.apiName}/version/${values.version}`,
      );
    } catch (error) {
      console.error("Failed to create API:", error);
      form.setError("apiName", {
        type: "manual",
        message: "Failed to create API. Please try again.",
      });
    } finally {
      setIsSubmitting(false);
    }
  };

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-4 py-16 max-w-2xl">
        <h1 className="text-2xl font-semibold mb-2">Create a new API</h1>
        <p className="text-muted-foreground mb-8">
          Export worker functions as a REST API
        </p>

        <Form {...form}>
          <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-6">
            <FormField
              control={form.control}
              name="apiName"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>API Name</FormLabel>
                  <FormControl>
                    <Input
                      placeholder="Must be unique per project"
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name="version"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>Version</FormLabel>
                  <FormControl>
                    <Input
                      placeholder="Version prefix for your API"
                      {...field}
                    />
                  </FormControl>
                  <p className="text-sm text-muted-foreground">
                    Version prefix for your API
                  </p>
                  <FormMessage />
                </FormItem>
              )}
            />

            <div className="flex justify-between">
              <Button
                type="button"
                variant="secondary"
                onClick={() => navigate(-1)}
                disabled={isSubmitting}
              >
                <ArrowLeft className="mr-2 h-5 w-5" />
                Back
              </Button>
              <Button
                type="submit"
                disabled={isSubmitting}
                className="flex items-center space-x-2"
              >
                {isSubmitting ? (
                  <Loader2 className="mr-2 h-5 w-5 animate-spin" />
                ) : (
                  <PlusCircle className="mr-2 h-5 w-5" />
                )}
                {isSubmitting ? "Creating..." : "Create API"}
              </Button>
            </div>
          </form>
        </Form>
      </div>
    </ErrorBoundary>
  );
};

export default CreateAPI;
