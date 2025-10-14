import {
  Card,
  CardContent,
  CardDescription,
  CardTitle,
} from "@/components/ui/card.tsx";
import { z } from "zod";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from "@/components/ui/form.tsx";
import { Input } from "@/components/ui/input.tsx";
import { ArrowLeft } from "lucide-react";
import { Button } from "@/components/ui/button.tsx";
import { API } from "@/service";
import { useNavigate, useParams } from "react-router-dom";
import ErrorBoundary from "@/components/errorBoundary";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.tsx";
import { toast } from "@/hooks/use-toast.ts";
import { useEffect, useState } from "react";

// Form schema using zod for validation
const formSchema = z.object({
  name: z
    .string()
    .min(4, { message: "Component name must be at least 4 characters" })
    .regex(/^[a-zA-Z0-9_-]+:[a-zA-Z0-9_-]+$/, {
      message: "Name must be in package:componentName format",
    }),
  template: z.string({
    required_error: "Please select a template",
  }),
});

const CreateComponent = () => {
  const navigate = useNavigate();
  const { appId } = useParams();
  const [templates, setTemplates] = useState<
    { language: string; template: string; description: string }[]
  >([]);
  const [isLoadingTemplates, setIsLoadingTemplates] = useState(true);

  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      name: "",
      template: "",
    },
  });

  // Fetch templates on mount
  useEffect(() => {
    const fetchTemplates = async () => {
      try {
        setIsLoadingTemplates(true);
        const fetchedTemplates =
          await API.componentService.getComponentTemplates();
        setTemplates(fetchedTemplates);
      } catch (error) {
        console.error("Error fetching templates:", error);
        toast({
          title: "Error fetching templates",
          description: String(error),
          variant: "destructive",
        });
      } finally {
        setIsLoadingTemplates(false);
      }
    };

    fetchTemplates();
  }, []);

  async function onSubmit(values: z.infer<typeof formSchema>) {
    API.componentService
      .createComponent(appId!, values.name, values.template)
      .then(() => {
        toast({
          title: `Component ${values.name} created successfully!`,
          description:
            "Please deploy your component to see it on the dashboard",
          duration: 8000,
          variant: "default",
        });
        navigate(`/app/${appId}/components`);
      });
  }

  return (
    <ErrorBoundary>
      <div className="p-6 bg-background text-foreground w-full overflow-y-auto h-[90vh]">
        <Card className="max-w-5xl mx-auto border shadow-md rounded-lg p-6">
          <CardTitle className="text-2xl font-bold">
            Create a New Component
          </CardTitle>
          <CardDescription className="text-gray-500">
            Select a template to create your component
          </CardDescription>
          <CardContent className="pt-6">
            <Form {...form}>
              <form
                onSubmit={form.handleSubmit(onSubmit)}
                className="space-y-6"
              >
                <FormField
                  control={form.control}
                  name="name"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Component Name</FormLabel>
                      <FormControl>
                        <Input {...field} placeholder="package:componentName" />
                      </FormControl>
                      <FormDescription>
                        Enter name in package:componentName format
                      </FormDescription>
                    </FormItem>
                  )}
                />

                <FormField
                  control={form.control}
                  name="template"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Template</FormLabel>
                      <FormControl>
                        <Select
                          onValueChange={field.onChange}
                          value={field.value}
                          disabled={isLoadingTemplates}
                        >
                          <SelectTrigger className="w-full">
                            <SelectValue
                              placeholder={
                                isLoadingTemplates
                                  ? "Loading templates..."
                                  : "Select a template"
                              }
                            />
                          </SelectTrigger>
                          <SelectContent>
                            {templates.map(template => (
                              <SelectItem
                                key={template.template}
                                value={template.template}
                              >
                                <div className="flex flex-col">
                                  <span className="font-medium">
                                    {template.template}
                                  </span>
                                  <span className="text-xs text-muted-foreground">
                                    {template.description}
                                  </span>
                                </div>
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </FormControl>
                      <FormDescription>
                        Choose a template for your component
                      </FormDescription>
                    </FormItem>
                  )}
                />

                <div className="flex justify-between mt-6">
                  <Button
                    type="button"
                    variant="secondary"
                    onClick={() => navigate(-1)}
                  >
                    <ArrowLeft className="mr-2 h-5 w-5" />
                    Back
                  </Button>
                  <Button type="submit" className="px-6 py-2">
                    Create Component
                  </Button>
                </div>
              </form>
            </Form>
          </CardContent>
        </Card>
      </div>
    </ErrorBoundary>
  );
};

export default CreateComponent;
