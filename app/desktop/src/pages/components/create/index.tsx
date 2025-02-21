import {
  Card,
  CardContent,
  CardDescription,
  CardTitle,
} from "@/components/ui/card.tsx";
import { z } from "zod";
import { useForm } from "react-hook-form";
import { DndProvider } from "react-dnd";
import JSZip from "jszip";
import { HTML5Backend } from "react-dnd-html5-backend";
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
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { ArrowLeft, Database, FileUp, Zap } from "lucide-react";
import { useRef, useState } from "react";
import { Button } from "@/components/ui/button.tsx";
import { API } from "@/service";
import { useNavigate } from "react-router-dom";
import ErrorBoundary from "@/components/errorBoundary";
import { FileManager, FileItem } from "./fileManager";

const COMPONENT_TYPES = [
  {
    value: "Durable",
    label: "Durable",
    icon: <Database className="h-5 w-5 text-gray-600" />,
    description:
      "Workers are persistent and executed with transactional guarantees. Ideal for stateful and high-reliability use cases.",
  },
  {
    value: "Ephemeral",
    label: "Ephemeral",
    icon: <Zap className="h-5 w-5 text-gray-600" />,
    description:
      "Workers are transient and executed normally. Ideal for stateless and low-reliability use cases.",
  },
];

const formSchema = z.object({
  name: z
    .string()
    .min(4, { message: "Component name must be at least 4 characters" })
    .optional(),
  type: z.enum(["Durable", "Ephemeral"]),
  component: z.instanceof(File).refine(file => file.size < 50000000, {
    message: "Component file must be less than 50MB.",
  }),
});

const CreateComponent = () => {
  const [file, setFile] = useState<File | null>(null);
  const [fileSystem, setFileSystem] = useState<FileItem[] | []>([]);
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

  async function addFilesToZip(zipFolder: JSZip, parentId: string | null) {
    const children = fileSystem.filter(file => file.parentId === parentId);
    for (const child of children) {
      if (child.type === "folder") {
        const folder = zipFolder.folder(child.name);
        if (folder) {
          await addFilesToZip(folder, child.id);
        }
      } else if (child.type === "file") {
        if (child.fileObject) {
          zipFolder.file(child.name, child.fileObject);
        }
      }
    }
  }

  // Recursive helper function to compute full path of a file item.
  function getFullPath(file: FileItem, allFiles: FileItem[]): string {
    if (!file.parentId) return `/${file.name}`;
    const parent = allFiles.find(f => f.id === file.parentId);
    if (!parent) return file.name;
    return `${getFullPath(parent, allFiles)}/${file.name}`;
  }

  function captureFileMetadata(allFiles: FileItem[]) {
    const filesPath: { path: string; permissions: string }[] = [];
    allFiles.forEach(file => {
      if (file.type != "folder") {
        filesPath.push({
          path: getFullPath(file, allFiles),
          permissions: file.isLocked ? "read-only" : "read-write",
        });
      }
    });
    return { values: filesPath };
  }

  async function onSubmit(values: z.infer<typeof formSchema>) {
    const formData = new FormData();
    formData.append("name", values.name!);
    formData.append("component", file!);
    formData.append("componentType", values.type!);
    // file system to zip
    const zip = new JSZip();
    await addFilesToZip(zip, null);
    const blob = await zip.generateAsync({ type: "blob" });
    formData.append(
      "filesPermissions",
      JSON.stringify(captureFileMetadata(fileSystem)),
    );
    formData.append("files", blob, "temp.zip");
    API.createComponent(formData).then(res => {
      if (res?.versionedComponentId?.componentId) {
        navigate(`/components/${res.versionedComponentId.componentId}`);
      }
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
            Components are the building blocks
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
                        <Input {...field} placeholder="Enter component name" />
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
                          value={field.value} // Controlled value
                          onValueChange={field.onChange} // Update value on change
                        >
                          {COMPONENT_TYPES.map(type => (
                            <FormItem
                              key={type.value}
                              className="flex items-center space-x-3 p-3 border rounded-lg cursor-pointer hover:bg-accent"
                            >
                              <FormControl>
                                <label className="flex items-center space-x-2 cursor-pointer w-full">
                                  <RadioGroupItem
                                    value={type.value}
                                    checked={field.value === type.value} // Ensure the correct item is checked
                                  />
                                  <div className="flex flex-col">
                                    <div className="flex items-center space-x-2">
                                      {type.icon}
                                      <span className="font-medium">
                                        {type.label}
                                      </span>
                                    </div>
                                    <p className="text-sm text-gray-600">
                                      {type.description}
                                    </p>
                                  </div>
                                </label>
                              </FormControl>
                            </FormItem>
                          ))}
                        </RadioGroup>
                      </FormControl>
                    </FormItem>
                  )}
                />

                <FormField
                  control={form.control}
                  name="component"
                  render={({ field: { onChange } }) => (
                    <FormItem>
                      <FormLabel>Component File</FormLabel>
                      <FormControl>
                        <div
                          className="border-2 border-dashed border-gray-300 rounded-lg p-6 text-center cursor-pointer hover:border-gray-400"
                          onClick={() => fileInputRef?.current?.click()}
                        >
                          <FileUp className="h-8 w-8 text-gray-500 mb-2 mx-auto" />
                          <Input
                            type="file"
                            accept=".wasm,application/wasm"
                            className="hidden"
                            ref={fileInputRef}
                            onChange={event => {
                              const file = event.target.files?.[0];
                              if (file) {
                                setFile(file);
                                onChange(file);
                              }
                            }}
                          />
                          <p className="text-sm text-gray-500">
                            File up to 50MB
                          </p>
                          <p className="font-medium mt-2">
                            {file ? file.name : "Upload WASM File"}
                          </p>
                        </div>
                      </FormControl>
                    </FormItem>
                  )}
                />

                <DndProvider backend={HTML5Backend}>
                  <FileManager files={fileSystem} setFiles={setFileSystem} />
                </DndProvider>

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
