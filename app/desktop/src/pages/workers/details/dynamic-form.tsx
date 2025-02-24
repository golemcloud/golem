import type React from "react";
import { useEffect, useState } from "react";
import { ComponentExportFunction } from "@/types/component";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { CircleSlash2, Info, Play, TimerReset } from "lucide-react";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  parseToJsonEditor,
  parseTooltipTypesData,
  safeFormatJSON,
  validateJsonStructure,
} from "@/lib/worker";
import { CodeBlock, dracula } from "react-code-blocks";
import { Textarea } from "@/components/ui/textarea";
import { Button } from "@/components/ui/button";
import { sanitizeInput } from "@/lib/utils";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type FormData = Record<string, any>;
type FieldType = {
  name: string;
  typ: {
    type: string;
    inner?: FieldType["typ"];
    cases?: string[];
  };
};

export const nonStringPrimitives = [
  "S64",
  "S32",
  "S16",
  "S8",
  "U64",
  "U32",
  "U16",
  "U8",
  "Bool",
  "Enum",
];

export const DynamicForm = ({
  functionDetails,
  onInvoke,
  resetResult,
}: {
  functionDetails: ComponentExportFunction;
  onInvoke: (args: unknown[]) => void;
  resetResult: () => void;
}) => {
  const [formData, setFormData] = useState<FormData>({});
  const [errors, setErrors] = useState<Record<string, string>>({});

  useEffect(() => {
    initialFormData();
  }, [functionDetails]);

  const initialFormData = () => {
    const initialData = functionDetails.parameters.reduce((acc, field) => {
      if (field.typ.type === "Str" || field.typ.type === "Chr") {
        acc[field.name] = "";
      } else if (!nonStringPrimitives.includes(field.typ.type)) {
        acc[field.name] = JSON.stringify(
          parseToJsonEditor({
            parameters: [{ ...field }],
            name: "",
            results: [],
          })[0],
          null,
          2,
        );
      }
      return acc;
    }, {} as FormData);
    setFormData(initialData);
    setErrors({});
  };

  const handleInputChange = (name: string, value: unknown) => {
    setFormData(prevData => ({ ...prevData, [name]: value }));
    setErrors(prevErrors => {
      const updatedErrors = { ...prevErrors };
      delete updatedErrors[name];
      return updatedErrors;
    });
    resetResult();
  };

  const validateForm = (): Record<string, string> => {
    const validationErrors: Record<string, string> = {};
    functionDetails.parameters.forEach(field => {
      let value = formData[field.name];
      if (nonStringPrimitives.includes(field.typ.type) && value === undefined) {
        validationErrors[field.name] = `${field.name} is required`;
      } else {
        if (
          !nonStringPrimitives.includes(field.typ.type) &&
          field.typ.type !== "Str" &&
          field.typ.type !== "Chr"
        ) {
          try {
            const sanitizedValue = sanitizeInput(value);
            value = JSON.parse(sanitizedValue);
          } catch (error) {
            validationErrors[field.name] = `${field.name} is not a valid JSON`;
            return null;
          }
        } else if (
          ["S64", "S32", "S16", "S8", "U64", "U32", "U16", "U8"].includes(
            field.typ.type,
          )
        ) {
          value = Number.parseInt(value);
        } else if (value !== undefined) {
          if (
            ["S64", "S32", "S16", "S8", "U64", "U32", "U16", "U8"].includes(
              field.typ.type,
            )
          ) {
            value = Number.parseInt(value);
          } else if (field.typ.type === "Bool") {
            value = Boolean(value);
          }
        }
        const error = validateJsonStructure(value, field);
        if (error) {
          validationErrors[field.name] = error;
        }
      }
    });
    return validationErrors;
  };

  const handleSubmit = () => {
    const validationErrors = validateForm();
    if (Object.keys(validationErrors).length > 0) {
      setErrors(validationErrors);
    } else {
      const result: unknown[] = [];
      functionDetails.parameters.forEach(field => {
        const value = formData[field.name] || "";
        if (
          !nonStringPrimitives.includes(field.typ.type) &&
          field.typ.type !== "Str" &&
          field.typ.type !== "Chr"
        ) {
          try {
            const sanitizedValue = sanitizeInput(value);
            result.push(JSON.parse(sanitizedValue));
          } catch (error) {
            console.error(`Error parsing JSON for field ${field.name}:`, error);
          }
        } else if (
          ["S64", "S32", "S16", "S8", "U64", "U32", "U16", "U8"].includes(
            field.typ.type,
          )
        ) {
          result.push(Number.parseInt(value));
        } else if (value !== undefined) {
          if (
            ["S64", "S32", "S16", "S8", "U64", "U32", "U16", "U8"].includes(
              field.typ.type,
            )
          ) {
            result.push(Number.parseInt(value));
          } else if (field.typ.type === "Bool") {
            result.push(Boolean(value));
          } else {
            result.push(value);
          }
        }
      });
      onInvoke(result);
    }
  };

  const buildInput = (field: FieldType, isOptional: boolean) => {
    const { name, typ } = field;
    const type = isOptional ? typ.inner?.type : typ.type;
    const value = formData[name] ?? "";

    switch (type) {
      case "S64":
      case "S32":
      case "S16":
      case "S8":
        return (
          <Input
            type="number"
            step="1"
            value={value}
            className={errors[name] ? "border-red-500" : ""}
            onChange={e => handleInputChange(name, e.target.value)}
          />
        );
      case "U64":
      case "U32":
      case "U16":
      case "U8":
        return (
          <Input
            type="number"
            min="0"
            value={value}
            className={errors[name] ? "border-red-500" : ""}
            onChange={e => {
              handleInputChange(name, e.target.value);
            }}
          />
        );
      case "Str":
      case "Chr":
        return (
          <Input
            type="text"
            value={value}
            className={errors[name] ? "border-red-500" : ""}
            onChange={e => handleInputChange(name, e.target.value)}
          />
        );
      case "Bool":
        return (
          <RadioGroup
            value={value}
            onValueChange={checked => handleInputChange(name, checked)}
          >
            <div className="flex items-center space-x-2">
              <RadioGroupItem value="true" id="r1" />
              <Label htmlFor="r1">True</Label>
            </div>
            <div className="flex items-center space-x-2">
              <RadioGroupItem value="false" id="r2" />
              <Label htmlFor="r2">False</Label>
            </div>
          </RadioGroup>
        );
      case "Enum":
        return (
          <Select
            value={value}
            onValueChange={selectedValue =>
              handleInputChange(name, selectedValue)
            }
          >
            <SelectTrigger>
              <SelectValue placeholder="Select an option" />
            </SelectTrigger>
            <SelectContent>
              {(typ.cases || []).map(option => (
                <SelectItem key={option} value={option}>
                  {option}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        );
      default:
        return (
          <Textarea
            value={value}
            onChange={e => {
              const newValue = safeFormatJSON(e.target.value);
              handleInputChange(name, newValue);
            }}
            className={`min-h-[400px] font-mono text-sm mt-2 ${
              errors[name] ? "border-red-500" : ""
            }`}
          />
        );
    }
  };

  const renderField = (field: FieldType): React.ReactNode => {
    const { name, typ } = field;
    const isOptional = typ.type === "Option";
    const dataType = typ.type;

    const parsedType = parseTooltipTypesData({
      parameters: [
        {
          ...field,
          type: "",
        },
      ],
      name: "",
      results: [],
    });

    return (
      <div key={name} className="mb-4">
        <Label>
          <div className="items-center text-center flex">
            <div>{name}</div>
            {isOptional && <div className="ml-2 text-zinc-400">(Optional)</div>}
            <div className="text-emerald-400 inline-flex items-center mr-2">
              :{dataType}
            </div>

            <Popover>
              <PopoverTrigger asChild>
                <button
                  className="p-1 hover:bg-muted rounded-full transition-colors"
                  aria-label="Show interpolation info"
                >
                  <Info className="w-4 h-4 text-muted-foreground" />
                </button>
              </PopoverTrigger>
              <PopoverContent
                className="w-[500px] font-mono text-[13px] bg-zinc-900 border-zinc-700 text-zinc-100 p-0 max-h-[500px] overflow-scroll"
                side="right"
                sideOffset={5}
              >
                <CodeBlock
                  text={JSON.stringify(parsedType?.[0], null, 2)}
                  language="json"
                  theme={dracula}
                />
              </PopoverContent>
            </Popover>
          </div>
        </Label>
        <div className="py-2">
          <div>{buildInput(field, isOptional)}</div>
          {errors[field.name] && (
            <div className="text-red-500 text-sm mt-2">
              {errors[field.name]}
            </div>
          )}
        </div>
      </div>
    );
  };

  return (
    <div>
      <Card className="w-full">
        <form>
          <CardContent className="p-6">
            {functionDetails.parameters.length > 0 ? (
              functionDetails.parameters.map(parameter =>
                renderField(parameter as FieldType),
              )
            ) : (
              <div className="flex flex-col items-center justify-center text-center gap-4">
                <div>
                  <CircleSlash2 className="h-12 w-12 text-muted-foreground" />
                </div>
                <div>No Parameters</div>
                <div className="text-muted-foreground">
                  This function has no parameters. You can invoke it without any
                  arguments.
                </div>
              </div>
            )}
          </CardContent>
        </form>
      </Card>
      <div className="flex gap-4 justify-end mt-4">
        <Button
          variant="outline"
          onClick={initialFormData}
          className="text-primary hover:bg-primary/10 hover:text-primary"
        >
          <TimerReset className="h-4 w-4 mr-1" />
          Reset
        </Button>
        <Button onClick={handleSubmit}>
          <Play className="h-4 w-4 mr-1" />
          Invoke
        </Button>
      </div>
    </div>
  );
};
