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
  FormMessage,
} from "@/components/ui/form.tsx";
import { Input } from "@/components/ui/input.tsx";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Database, FileUp, Zap, ArrowLeft } from "lucide-react";
import { useRef, useState } from "react";
import { Button } from "@/components/ui/button.tsx";
import { API } from "@/service";
import { useNavigate } from "react-router-dom";
import ErrorBoundary from "@/components/errorBoundary";

const COMPONENT_TYPES = [
  {
    value: "Durable",
    label: "Durable",
    icon: <Database className="h-5 w-5 text-gray-600" />,
    description:
      "Workers are persistent and executed with transactional guarantees\nIdeal for stateful and high-reliability use cases",
  },
  {
    value: "Ephemeral",
    label: "Ephemeral",
    icon: <Zap className="h-5 w-5 text-gray-600" />,
    description:
      "Workers are transient and executed normally\nIdeal for stateless and low-reliability use cases",
  },
];

const formSchema = z.object({
  name: z
    .string()
    .min(4, {
      message: "Component name must be at least 4 characters",
    })
    .optional(),
  type: z.enum(["Durable", "Ephemeral"]),
  component: z.instanceof(File).refine((file) => file.size < 50000000, {
    message: "Your resume must be less than 50MB.",
  }),
});

const CreateComponent = () => {
  const [file, setFile] = useState<File | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const navigate = useNavigate();
  const form = useForm<z.infer<typeof formSchema>>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      name: "",
      type: undefined,
      component: undefined,
    },
  });

  function onSubmit(values: z.infer<typeof formSchema>) {
    const formData = new FormData();
    formData.append("name", values.name!);
    formData.append("component", file!);
    formData.append("componentType", values.type!);
    API.createComponent(formData).then((res) => {
      if (res?.versionedComponentId?.componentId) {
        navigate(`/components/${res.versionedComponentId.componentId}`);
      }
    });
  }

  return (
    <ErrorBoundary>
      <div className="p-4 bg-background text-foreground mx-auto max-w-7xl px-6 lg:px-8 py-4">
        <Card
          className="max-w-2xl mx-auto border-0 shadow-none overflow-scroll h-[80vh]"
          key={"component.componentName"}
        >
          <CardTitle>
            <h1 className="text-2xl font-semibold mb-1">
              Create a new Component
            </h1>
          </CardTitle>
          <CardDescription>
            <p className="text-sm text-gray-400">
              Components are the building blocks
            </p>
          </CardDescription>
          <CardContent className="p-6">
            <Form {...form}>
              <form
                onSubmit={form.handleSubmit(onSubmit)}
                className="space-y-8"
              >
                <FormField
                  control={form.control}
                  name="name"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Name</FormLabel>
                      <FormControl>
                        <Input {...field} />
                      </FormControl>
                      <FormDescription>
                        The name must be unique for this component.
                      </FormDescription>
                    </FormItem>
                  )}
                />
                <FormField
                  control={form.control}
                  name="type"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>Component Type</FormLabel>
                      <FormControl>
                        <RadioGroup
                          onValueChange={field.onChange}
                          defaultValue={field.value}
                          {...field}
                        >
                          {COMPONENT_TYPES.map((type) => (
                            // <div key={type} className="flex items-center space-x-2">
                            <FormItem
                              key={type.value}
                              className="flex items-start space-x-3 p-3 border rounded cursor-pointer hover:bg-accent"
                            >
                              <FormControl>
                                <RadioGroupItem
                                  value={type.value}
                                  className="flex items-center justify-center self-center"
                                />
                              </FormControl>
                              <FormLabel className="font-normal">
                                <div className="flex items-center space-x-2">
                                  {type.icon}
                                  <span className="font-medium">
                                    {type.label}
                                  </span>
                                </div>
                                <p className="text-sm text-gray-600 mt-1">
                                  {type.description}
                                </p>
                              </FormLabel>
                            </FormItem>
                          ))}
                        </RadioGroup>
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={form.control}
                  name="component"
                  render={({ field: { onChange } }) => (
                    <FormItem>
                      <FormLabel>Component</FormLabel>
                      <FormControl>
                        <div
                          className="border-2 border-dashed border-gray-200 rounded-lg p-8 cursor-pointer hover:border-gray-400"
                          onClick={() => fileInputRef?.current?.click()}
                        >
                          <div className="flex flex-col items-center justify-center text-center">
                            <FileUp className="h-8 w-8 text-gray-400 mb-3" />
                            <Input
                              type="file"
                              accept="application/wasm,.wasm"
                              className="hidden"
                              ref={fileInputRef}
                              onChange={(event) => {
                                const file = event.target.files?.[0];
                                if (file) {
                                  setFile(file);
                                  onChange(file);
                                }
                              }}
                            />
                            <p className="text-sm text-gray-500 mb-4">
                              File up to 50MB
                            </p>
                            <p className="font-medium mb-1">
                              {file ? file.name : "Upload Component WASM"}
                            </p>
                          </div>
                        </div>
                      </FormControl>
                    </FormItem>
                  )}
                />
                <div className="flex justify-between">
                  <Button
                    type="button"
                    variant="secondary"
                    onClick={() => navigate(-1)}
                  >
                    <ArrowLeft className="mr-2 h-5 w-5" />
                    Back
                  </Button>
                  <Button type="submit">Create Component</Button>
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
