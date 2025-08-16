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

// Language template options
const LANGUAGE_TEMPLATES = [
  // C
  { value: "c", label: "C: Default component template" },
  { value: "c/example-http", label: "C: Example - Stateful with WASI HTTP" },
  // Go
  { value: "go", label: "Go: Default component template" },
  { value: "go/wasi-http", label: "Go: WASI HTTP handler" },
  // JavaScript
  { value: "js", label: "JavaScript: Default component template" },
  { value: "js/example-fetch", label: "JavaScript: Example with fetch" },
  { value: "js/wasi-http", label: "JavaScript: WASI HTTP handler" },
  // Python
  { value: "python", label: "Python: Default component template" },
  { value: "python/wasi-http", label: "Python: WASI HTTP handler" },
  // Rust
  { value: "rust/async", label: "Rust: Async with tokio support" },
  { value: "rust", label: "Rust: Default component template" },
  {
    value: "rust/example-shopping-cart",
    label: "Rust: Example - Stateful shopping cart",
  },
  {
    value: "rust/example-todo-list",
    label: "Rust: Example - Stateful todo list",
  },
  { value: "rust/minimal", label: "Rust: Minimal with no dependencies" },
  { value: "rust/wasi-http", label: "Rust: WASI HTTP handler" },
  // TypeScript
  { value: "ts", label: "TypeScript: Default component template" },
  { value: "ts/example-fetch", label: "TypeScript: Example using fetch" },
  // Zig
  { value: "zig", label: "Zig: Default component template" },
  // Scala.js
  { value: "scala", label: "Scala.js: Default component template" },
  // MoonBit
  { value: "moonbit", label: "MoonBit: Default component template" },
];

// Group templates by language
const GROUPED_TEMPLATES = {
  C: LANGUAGE_TEMPLATES.filter(t => t.value.startsWith("c")),
  Go: LANGUAGE_TEMPLATES.filter(t => t.value.startsWith("go")),
  JavaScript: LANGUAGE_TEMPLATES.filter(t => t.value.startsWith("js")),
  Python: LANGUAGE_TEMPLATES.filter(t => t.value.startsWith("python")),
  Rust: LANGUAGE_TEMPLATES.filter(t => t.value.startsWith("rust")),
  TypeScript: LANGUAGE_TEMPLATES.filter(t => t.value.startsWith("ts")),
  Zig: LANGUAGE_TEMPLATES.filter(t => t.value === "zig"),
  "Scala.js": LANGUAGE_TEMPLATES.filter(t => t.value === "scala"),
  MoonBit: LANGUAGE_TEMPLATES.filter(t => t.value === "moonbit"),
};

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
  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      name: "",
      template: "",
    },
  });
  const { appId } = useParams();

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
                        >
                          <SelectTrigger className="w-full">
                            <SelectValue placeholder="Select a template" />
                          </SelectTrigger>
                          <SelectContent>
                            {Object.entries(GROUPED_TEMPLATES).map(
                              ([language, templates]) => (
                                <div key={language} className="mb-2">
                                  <h3 className="font-semibold px-2 py-1 bg-muted text-muted-foreground text-sm">
                                    {language}
                                  </h3>
                                  {templates.map(template => (
                                    <SelectItem
                                      key={template.value}
                                      value={template.value}
                                    >
                                      {template.label}
                                    </SelectItem>
                                  ))}
                                </div>
                              ),
                            )}
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
