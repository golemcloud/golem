import * as Yup from "yup";

import { ErrorMessage, Field, Form, Formik } from "formik";
import { Loader2, Plus, Server, Settings, X } from "lucide-react";

import toast from "react-hot-toast";
import { useComponents } from "../../api/components";
import { useCreatePlugin } from "../../api/plugins";
import { useState } from "react";

type PluginType = "OplogProcessor" | "ComponentTransformer";

interface CreatePluginModalProps {
  isOpen: boolean;
  onClose: () => void;
}

interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label: string;
  error?: string;
  name: string;
}

// Validation Schema
const validationSchema = Yup.object().shape({
  name: Yup.string()
    .required("Plugin name is required")
    .min(3, "Plugin name must be at least 3 characters")
    .matches(/^[a-zA-Z0-9-_ ]+$/, "Only alphanumeric characters, hyphens, and underscores allowed"),
  version: Yup.string()
    .required("Version is required")
    .matches(
      /^\d+\.\d+\.\d+$/,
      "Version must be in semantic versioning format (e.g., 1.0.0)"
    ),
  description: Yup.string()
    .required("Description is required")
    .min(10, "Description must be at least 10 characters"),
  homepage: Yup.string()
    .url("Must be a valid URL")
    .required("Homepage URL is required"),
  type: Yup.string()
    .required("Plugin type is required")
    .oneOf(["OplogProcessor", "ComponentTransformer"]),
  // Conditional validation based on plugin type
  componentId: Yup.string().when("type", {
    is: "OplogProcessor",
    then: (schema) => schema.required("Component is required"),
  }),
  componentVersion: Yup.number().when("type", {
    is: "OplogProcessor",
    then: (schema) => schema.required("Component version is required"),
  }),
  jsonSchema: Yup.string().when("type", {
    is: "ComponentTransformer",
    then: (schema) => schema.required("JSON Schema is required")
      .test("is-valid-json", "Must be valid JSON", (value) => {
        if (!value) return false;
        try {
          JSON.parse(value);
          return true;
        } catch {
          return false;
        }
      }),
  }),
  validateUrl: Yup.string().when("type", {
    is: "ComponentTransformer",
    then: (schema) => schema.required("Validate URL is required").url("Must be a valid URL"),
  }),
  transformUrl: Yup.string().when("type", {
    is: "ComponentTransformer",
    then: (schema) => schema.required("Transform URL is required").url("Must be a valid URL"),
  }),
});

const Input: React.FC<InputProps> = ({ label, error, name, ...props }) => (
  <div>
    <label htmlFor={name} className="block text-sm font-medium mb-1.5 text-foreground/80">
      {label}
    </label>
    <Field
      id={name}
      name={name}
      {...props}
      className="w-full px-3 md:px-4 py-2 md:py-2.5 bg-card/50 rounded-lg border border-input 
               focus:border-primary focus:ring-1 focus:ring-primary outline-none transition duration-200
               disabled:opacity-50 disabled:cursor-not-allowed text-sm md:text-base"
    />
    <ErrorMessage
      name={name}
      component="div"
      className="mt-1 flex items-center gap-1 text-destructive text-xs md:text-sm"
    />
  </div>
);

export const CreatePluginModal = ({
  isOpen,
  onClose,
}: CreatePluginModalProps) => {
  const { data: components } = useComponents();
  const createPlugin = useCreatePlugin();
  const [isSubmitting, setIsSubmitting] = useState(false);

  const initialValues = {
    name: "",
    version: "",
    description: "",
    homepage: "",
    type: "ComponentTransformer" as PluginType,
    componentId: "",
    componentVersion: 0,
    jsonSchema: "",
    validateUrl: "",
    transformUrl: "",
  };

  const handleSubmit = async (values: typeof initialValues) => {
    setIsSubmitting(true);

    const pluginData = {
      name: values.name,
      version: values.version,
      description: values.description,
      specs:
        values.type === "OplogProcessor"
          ? {
              type: "OplogProcessor" as const,
              componentId: values.componentId,
              componentVersion: values.componentVersion,
            }
          : {
              type: "ComponentTransformer" as const,
              jsonSchema: values.jsonSchema,
              validateUrl: values.validateUrl,
              transformUrl: values.transformUrl,
            },
      scope: {
        type: "Global" as const,
      },
      icon: [0],
      homepage: values.homepage,
    };

    try {
      await createPlugin.mutateAsync(pluginData);
      toast.success("Plugin created successfully");
      onClose();
    } catch (error) {
      console.error(error);
      toast.error("Failed to create plugin");
    } finally {
      setIsSubmitting(false);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-background/60 flex items-start md:items-center justify-center p-4 z-50 backdrop-blur-sm overflow-y-auto pt-48">
      <div className="bg-card rounded-xl p-4 md:p-6 w-full max-w-2xl shadow-xl my-4 md:my-0 border border-border/10">
        <Formik
          initialValues={initialValues}
          validationSchema={validationSchema}
          onSubmit={handleSubmit}
        >
          {({ values, setFieldValue }) => (
            <Form className="space-y-6">
              <div className="flex justify-between items-start mb-4">
                <div className="flex items-center gap-3">
                  <div className="p-2 rounded-md bg-primary/10 text-primary">
                    <Plus size={20} />
                  </div>
                  <div>
                    <h2 className="text-xl font-semibold">Create New Plugin</h2>
                    <p className="text-sm text-muted-foreground mt-1">
                      Configure your plugin settings
                    </p>
                  </div>
                </div>
                <button
                  type="button"
                  onClick={onClose}
                  className="text-muted-foreground hover:text-foreground p-1 hover:bg-muted/50 
                    rounded-md transition-colors"
                >
                  <X size={20} />
                </button>
              </div>

              <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                <Input
                  label="Plugin Name"
                  name="name"
                  type="text"
                  placeholder="Enter plugin name"
                  disabled={isSubmitting}
                />
                <Input
                  label="Version"
                  name="version"
                  type="text"
                  placeholder="e.g., 1.0.0"
                  disabled={isSubmitting}
                />
              </div>

              <Input
                label="Description"
                name="description"
                type="text"
                placeholder="Brief description of your plugin"
                disabled={isSubmitting}
              />

              <Input
                label="Homepage"
                name="homepage"
                type="url"
                placeholder="https://"
                disabled={isSubmitting}
              />

              <div className="space-y-2">
                <label className="block text-sm font-medium text-foreground/80">
                  Plugin Type
                </label>
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                  {[
                    {
                      value: "OplogProcessor",
                      label: "Oplog Processor",
                      icon: Server,
                    },
                    {
                      value: "ComponentTransformer",
                      label: "Component Transformer",
                      icon: Settings,
                    },
                  ].map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      onClick={() => setFieldValue("type", option.value)}
                      className={`flex items-center gap-3 p-4 rounded-lg border-2 transition-all
                        ${
                          values.type === option.value
                            ? "border-primary bg-primary/10"
                            : "border-input hover:border-input/80"
                        }`}
                      disabled={isSubmitting}
                    >
                      <option.icon
                        className={
                          values.type === option.value
                            ? "text-primary"
                            : "text-muted-foreground"
                        }
                        size={20}
                      />
                      <span>{option.label}</span>
                    </button>
                  ))}
                </div>
              </div>

              {values.type === "OplogProcessor" ? (
                <div className="space-y-4 border-t border-border/10 pt-4">
                  <div>
                    <label className="block text-sm font-medium mb-1.5 text-foreground/80">
                      Component and Version
                    </label>
                    <Field
                      as="select"
                      name="componentId"
                      className="w-full px-3 md:px-4 py-2 md:py-2.5 bg-card/50 rounded-lg border border-input 
                        focus:border-primary focus:ring-1 focus:ring-primary outline-none"
                      disabled={isSubmitting}
                      onChange={(e: React.ChangeEvent<HTMLSelectElement>) => {
                        const [componentId, version] = e.target.value.split(":");
                        setFieldValue("componentId", componentId);
                        setFieldValue("componentVersion", Number(version));
                      }}
                    >
                      <option value="">Select a component</option>
                      {components?.map((component) => (
                        <option
                          key={`${component.versionedComponentId.componentId}:${component.versionedComponentId.version}`}
                          value={`${component.versionedComponentId.componentId}:${component.versionedComponentId.version}`}
                        >
                          {component.componentName} (v
                          {component.versionedComponentId.version})
                        </option>
                      ))}
                    </Field>
                    <ErrorMessage
                      name="componentId"
                      component="div"
                      className="mt-1 text-destructive text-sm"
                    />
                  </div>
                </div>
              ) : (
                <div className="space-y-4 border-t border-border/10 pt-4">
                  <div>
                    <label className="block text-sm font-medium mb-1.5 text-foreground/80">
                      JSON Schema
                    </label>
                    <Field
                      as="textarea"
                      name="jsonSchema"
                      className="w-full px-3 md:px-4 py-2 md:py-2.5 bg-card/50 rounded-lg border border-input 
                        focus:border-primary outline-none font-mono text-sm h-32 resize-none"
                      placeholder="{}"
                      disabled={isSubmitting}
                    />
                    <ErrorMessage
                      name="jsonSchema"
                      component="div"
                      className="mt-1 text-destructive text-sm"
                    />
                  </div>
                  <Input
                    label="Validate URL"
                    name="validateUrl"
                    type="url"
                    placeholder="https://"
                    disabled={isSubmitting}
                  />
                  <Input
                    label="Transform URL"
                    name="transformUrl"
                    type="url"
                    placeholder="https://"
                    disabled={isSubmitting}
                  />
                </div>
              )}

              <div className="flex flex-col-reverse sm:flex-row sm:justify-end items-stretch sm:items-center gap-3 pt-4 border-t border-border/10">
                <button
                  type="button"
                  onClick={onClose}
                  className="px-4 py-2 text-sm bg-muted hover:bg-muted/80 rounded-lg transition-colors
                    disabled:opacity-50 w-full sm:w-auto"
                  disabled={isSubmitting}
                >
                  Cancel
                </button>
                <button
                  type="submit"
                  disabled={isSubmitting}
                  className="px-4 py-2 text-sm bg-primary text-primary-foreground rounded-lg hover:bg-primary/90 
                    disabled:opacity-50 transition-colors flex items-center justify-center gap-2 w-full sm:w-auto"
                >
                  {isSubmitting ? (
                    <>
                      <Loader2 size={16} className="animate-spin" />
                      <span>Creating...</span>
                    </>
                  ) : (
                    <>
                      <Plus size={16} />
                      <span>Create Plugin</span>
                    </>
                  )}
                </button>
              </div>
            </Form>
          )}
        </Formik>
      </div>
    </div>
  );
};