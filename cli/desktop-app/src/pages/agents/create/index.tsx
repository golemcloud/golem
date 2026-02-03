// @ts-nocheck
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { z } from "zod";
import { useFieldArray, useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from "@/components/ui/form";
import { API } from "@/service";
import { ArrowLeft, Loader2 } from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { useEffect, useState } from "react";
import { AgentTypeSchema } from "@/types/agent-types";
import { RecursiveParameterInput } from "@/components/invoke/RecursiveParameterInput";
import { Typ } from "@/types/component";

// Convert PascalCase to kebab-case (e.g., CounterAgent -> counter-agent)
const toKebabCase = (str: string): string => {
  return str
    .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
    .replace(/([A-Z])([A-Z][a-z])/g, "$1-$2")
    .toLowerCase();
};

const formSchema = z.object({
  componentID: z.string(),
  agentTypeIndex: z.number().min(0, "Please select an agent type"),
  constructorParams: z.record(z.unknown()).optional(),
  env: z.array(
    z.object({
      key: z.string(),
      value: z.string(),
    }),
  ),
  args: z.array(z.string()),
});

const createEmptyValue = (type: Typ): unknown => {
  const typeStr = type.type?.toLowerCase();

  switch (typeStr) {
    case "str":
    case "chr":
      return "";
    case "bool":
      return false;
    case "record":
      // eslint-disable-next-line no-case-declarations
      const record: Record<string, unknown> = {};
      type.fields?.forEach(field => {
        record[field.name] = createEmptyValue(field.typ);
      });
      return record;
    case "list":
      return [];
    case "option":
      return null;
    case "variant":
      if (type.cases && type.cases.length > 0) {
        const firstCase = type.cases[0];
        if (typeof firstCase === "string") {
          return firstCase;
        } else {
          return { [firstCase.name]: createEmptyValue(firstCase.typ) };
        }
      }
      return null;
    case "enum":
      if (type.cases && type.cases.length > 0) {
        return type.cases[0];
      }
      return "";
    case "flags":
      return [];
    case "tuple":
      if (type.fields && type.fields.length > 0) {
        return type.fields.map(field => createEmptyValue(field.typ));
      }
      return [];
    case "f64":
    case "f32":
    case "u64":
    case "s64":
    case "u32":
    case "s32":
    case "u16":
    case "s16":
    case "u8":
    case "s8":
      return 0;
    default:
      return null;
  }
};

export default function CreateAgent() {
  const navigate = useNavigate();
  const { componentId, appId } = useParams();

  const [agentTypes, setAgentTypes] = useState<AgentTypeSchema[]>([]);
  const [selectedAgentType, setSelectedAgentType] =
    useState<AgentTypeSchema | null>(null);
  const [constructorValues, setConstructorValues] = useState<
    Record<string, unknown>
  >({});
  const [isLoading, setIsLoading] = useState(true);
  const [isSubmitting, setIsSubmitting] = useState(false);

  const form = useForm({
    resolver: zodResolver(formSchema),
    defaultValues: {
      componentID: componentId,
      agentTypeIndex: -1,
      constructorParams: {},
      env: [{ key: "", value: "" }],
      args: [""],
    },
  });

  const {
    fields: envFields,
    append: appendEnv,
    remove: removeEnv,
  } = useFieldArray({
    control: form.control,
    name: "env",
  });

  const {
    fields: argFields,
    append: appendArg,
    remove: removeArg,
  } = useFieldArray({
    control: form.control,
    name: "args",
  });

  useEffect(() => {
    const fetchAgentTypes = async () => {
      try {
        setIsLoading(true);
        const types = await API.agentService.getAgentTypesForComponent(
          appId!,
          componentId!,
        );

        // Group agent types by their kebab-case API name (deduplicate)
        const uniqueTypesMap = new Map<string, AgentTypeSchema>();
        types.forEach(type => {
          const kebabCaseName = toKebabCase(type.agentType.typeName);
          // Only keep the first occurrence of each unique type
          if (!uniqueTypesMap.has(kebabCaseName)) {
            uniqueTypesMap.set(kebabCaseName, type);
          }
        });
        setAgentTypes(Array.from(uniqueTypesMap.values()));
      } catch (error) {
        console.error("Failed to fetch agent types:", error);
      } finally {
        setIsLoading(false);
      }
    };

    fetchAgentTypes();
  }, [appId, componentId]);

  const handleAgentTypeChange = (index: string) => {
    const agentTypeIndex = parseInt(index);
    const agentType = agentTypes[agentTypeIndex];
    setSelectedAgentType(agentType || null);

    if (agentType && agentType.agentType) {
      // Initialize constructor parameter values
      const initialValues: Record<string, unknown> = {};

      // Access the inputSchema.elements for constructor parameters
      const constructorParams =
        agentType.agentType.constructor.inputSchema.elements || [];

      constructorParams.forEach(param => {
        // The parameter schema is in param.schema.elementType
        initialValues[param.name] = createEmptyValue(
          param.schema?.elementType || param.schema,
        );
      });

      setConstructorValues(initialValues);
      form.setValue("constructorParams", initialValues);
    } else {
      setConstructorValues({});
      form.setValue("constructorParams", {});
    }
  };
  const handleConstructorParamChange = (paramName: string, value: unknown) => {
    const newValues = { ...constructorValues, [paramName]: value };
    setConstructorValues(newValues);
    form.setValue("constructorParams", newValues);
  };

  async function onSubmit(values) {
    try {
      setIsSubmitting(true);
      const {
        componentID,
        _agentTypeIndex,
        constructorParams,
        env: envArray,
        args: argsArray,
      } = values;

      if (!selectedAgentType) {
        throw new Error("No agent type selected");
      }

      // Convert constructor params to array in the correct order
      const constructorElements =
        selectedAgentType.agentType.constructor.inputSchema.elements || [];
      const constructorParamsArray = constructorElements.map(
        param => constructorParams?.[param.name],
      );

      // Extract constructor parameter types in the same order
      const constructorParamTypes = constructorElements.map(
        param => param.schema?.elementType || param.schema,
      );
      // Convert env array to object, filtering out empty entries
      const envObject = envArray.reduce((acc, item) => {
        if (item.key && item.key.trim()) {
          acc[item.key] = item.value || "";
        }
        return acc;
      }, {});

      // Filter out empty arguments
      const filteredArgs = argsArray.filter(
        arg => arg && arg.trim().length > 0,
      );
      // Convert PascalCase typeName to kebab-case for CLI
      const agentTypeName = toKebabCase(selectedAgentType.agentType.typeName);

      const response = await API.agentService.createAgent(
        appId,
        componentID,
        agentTypeName,
        constructorParamsArray,
        constructorParamTypes,
        filteredArgs,
        envObject,
      );
      navigate(
        `/app/${appId}/components/${componentId}/agents/${response.worker_name}`,
      );
    } catch (error) {
      console.error("Failed to create agent:", error);
    } finally {
      setIsSubmitting(false);
    }
  }

  if (isLoading) {
    return (
      <div className="flex justify-center p-10">
        <Card className="w-full max-w-2xl border shadow-md p-6">
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-8 w-8 animate-spin" />
            <span className="ml-2">Loading agent types...</span>
          </div>
        </Card>
      </div>
    );
  }

  return (
    <div className="flex justify-center p-10">
      <Card className="w-full max-w-4xl border shadow-md p-6">
        <CardTitle className="text-xl font-bold">Create a New Agent</CardTitle>
        <CardDescription className="text-gray-500 mb-6">
          Select an agent type and configure its constructor parameters. The
          agent will be created with the specified type and parameters.
        </CardDescription>
        <CardContent>
          <Form {...form}>
            <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-6">
              {/* Agent Type Selection */}
              <FormField
                control={form.control}
                name="agentTypeIndex"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Agent Type</FormLabel>
                    <FormControl>
                      <Select
                        value={field.value >= 0 ? field.value.toString() : ""}
                        onValueChange={value => {
                          const index = parseInt(value);
                          field.onChange(index);
                          handleAgentTypeChange(value);
                        }}
                      >
                        <SelectTrigger>
                          <SelectValue placeholder="Select an agent type..." />
                        </SelectTrigger>
                        <SelectContent>
                          {agentTypes.map((agentTypeSchema, index) => (
                            <SelectItem key={index} value={index.toString()}>
                              <div className="flex flex-col">
                                <span className="font-medium">
                                  {agentTypeSchema.agentType.typeName}
                                </span>
                                <span className="text-sm text-muted-foreground">
                                  {agentTypeSchema.agentType.description}
                                </span>
                              </div>
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </FormControl>
                    <FormDescription>
                      Choose the type of agent to create. This determines the
                      available constructor parameters.
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />

              {/* Constructor Parameters */}

              {selectedAgentType &&
                selectedAgentType.agentType.constructor.inputSchema.elements
                  .length > 0 && (
                  <div className="space-y-4">
                    <div>
                      <Label className="text-base font-semibold">
                        Constructor Parameters
                      </Label>
                      <p className="text-sm text-muted-foreground mt-1">
                        {selectedAgentType.agentType.constructor.description}
                      </p>
                    </div>
                    <Card className="bg-muted/30">
                      <CardContent className="p-4 space-y-4">
                        {selectedAgentType.agentType.constructor.inputSchema.elements.map(
                          param => (
                            <RecursiveParameterInput
                              key={param.name}
                              name={param.name}
                              typeDef={
                                param.schema?.elementType || param.schema
                              }
                              value={constructorValues[param.name]}
                              onChange={(_, value) =>
                                handleConstructorParamChange(param.name, value)
                              }
                            />
                          ),
                        )}
                      </CardContent>
                    </Card>
                  </div>
                )}

              {/* Environment Variables */}
              <div>
                <FormLabel>Environment Variables</FormLabel>
                <div className="space-y-2 pt-2">
                  {envFields.map((field, index) => (
                    <div
                      key={field.name + field.componentId}
                      className="flex gap-2"
                    >
                      <Input
                        placeholder="Key"
                        {...form.register(`env.${index}.key`)}
                      />
                      <Input
                        placeholder="Value"
                        type="password"
                        {...form.register(`env.${index}.value`)}
                      />
                      <Button
                        type="button"
                        variant="destructive"
                        size="sm"
                        onClick={() => removeEnv(index)}
                      >
                        Remove
                      </Button>
                    </div>
                  ))}
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => appendEnv({ key: "", value: "" })}
                  >
                    Add Environment Variable
                  </Button>
                </div>
              </div>

              {/* Arguments */}
              <div>
                <FormLabel>Arguments</FormLabel>
                <div className="space-y-2 pt-2">
                  {argFields.map((field, index) => (
                    <div key={field.appId} className="flex gap-2">
                      <Input {...form.register(`args.${index}`)} />
                      <Button
                        type="button"
                        variant="destructive"
                        size="sm"
                        onClick={() => removeArg(index)}
                      >
                        Remove
                      </Button>
                    </div>
                  ))}
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => appendArg("")}
                  >
                    Add Argument
                  </Button>
                </div>
              </div>

              {/* Submit Buttons */}
              <div className="flex justify-between">
                <Button
                  type="button"
                  variant="secondary"
                  onClick={() => navigate(-1)}
                  disabled={isSubmitting}
                >
                  <ArrowLeft className="mr-2 h-5 w-5" /> Back
                </Button>
                <Button type="submit" disabled={isSubmitting}>
                  {isSubmitting ? (
                    <>
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      Creating...
                    </>
                  ) : (
                    "Create Agent"
                  )}
                </Button>
              </div>
            </form>
          </Form>
        </CardContent>
      </Card>
    </div>
  );
}
