import React from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Textarea } from "@/components/ui/textarea";
import { Card, CardContent } from "@/components/ui/card";
import { MinusCircle, PlusCircle } from "lucide-react";
import { Typ } from "@/types/component";

interface RecursiveParameterInputProps {
  name: string;
  typeDef: Typ;
  value: unknown;
  onChange: (path: string, value: unknown) => void;
  path?: string;
}

const TypeBadge = ({ type }: { type: string }) => (
  <span className="px-2 py-0.5 rounded-full text-xs bg-blue-500/10 text-blue-400 font-mono">
    {type}
  </span>
);

const createEmptyValue = (
  typeDef: Typ,
  visited = new Set<string>(),
): unknown => {
  const typeStr = typeDef.type?.toLowerCase();

  // Prevent infinite recursion for self-referencing types
  const typeKey = JSON.stringify(typeDef);
  if (visited.has(typeKey)) {
    return null;
  }
  visited.add(typeKey);

  switch (typeStr) {
    case "record": {
      const record: Record<string, unknown> = {};
      typeDef.fields?.forEach(field => {
        record[field.name] = createEmptyValue(field.typ, visited);
      });
      return record;
    }

    case "list":
      return [];

    case "option":
      return null;

    case "variant": {
      // For variants, create the first case as default
      if (typeDef.cases && typeDef.cases.length > 0) {
        const firstCase = typeDef.cases[0];
        if (!firstCase) return {};
        if (typeof firstCase === "string") {
          // Unit variant - just return the case name
          return firstCase;
        } else {
          // Variant with data - create object with case name as key
          return { [firstCase.name]: createEmptyValue(firstCase.typ, visited) };
        }
      }
      return null;
    }

    case "str":
    case "chr":
      return "";

    case "bool":
      return false;

    case "enum":
      if (typeDef.cases && typeDef.cases.length > 0) {
        return typeDef.cases[0];
      }
      return "";

    case "flags":
      // For flags, create empty array as default
      return [];

    case "result":
      // For results, create default ok value
      return { ok: typeDef.ok ? createEmptyValue(typeDef.ok, visited) : null };

    case "tuple":
      // For tuples, create array with empty values for each element
      if (typeDef.fields && typeDef.fields.length > 0) {
        return typeDef.fields.map(field =>
          createEmptyValue(field.typ, visited),
        );
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

export const RecursiveParameterInput: React.FC<
  RecursiveParameterInputProps
> = ({ name, typeDef, value, onChange, path = "" }) => {
  const currentPath = path ? `${path}.${name}` : name;

  const handleValueChange = (newValue: unknown) => {
    onChange(currentPath, newValue);
  };

  const renderInput = () => {
    const typeStr = typeDef.type?.toLowerCase();
    switch (typeStr) {
      case "record":
        return (
          <Card className="bg-card/60 border-border/20">
            <CardContent className="p-4 space-y-4">
              {typeDef.fields?.map(field => (
                <div key={field.name}>
                  <RecursiveParameterInput
                    name={field.name}
                    typeDef={field.typ}
                    value={(value as Record<string, unknown>)?.[field.name]}
                    onChange={(_, fieldValue) => {
                      const newValue = {
                        ...((value as Record<string, unknown>) || {}),
                      };
                      newValue[field.name] = fieldValue;
                      handleValueChange(newValue);
                    }}
                    path={currentPath}
                  />
                </div>
              ))}
            </CardContent>
          </Card>
        );

      case "variant":
        return (
          <div className="space-y-4">
            <Select
              value={(() => {
                if (typeof value === "string") return value;
                if (value && typeof value === "object" && value !== null) {
                  const entries = Object.entries(
                    value as Record<string, unknown>,
                  );
                  if (entries.length === 1) {
                    return entries[0]?.[0]; // Return the case name
                  }
                }
                return "";
              })()}
              onValueChange={selectedType => {
                const selectedCase = typeDef.cases?.find(
                  c => (typeof c === "string" ? c : c.name) === selectedType,
                );
                if (selectedCase) {
                  if (typeof selectedCase === "string") {
                    // Unit variant - just the case name
                    handleValueChange(selectedType);
                  } else {
                    // Variant with data - create object with case name as key
                    const caseValue = createEmptyValue(selectedCase.typ);
                    handleValueChange({ [selectedType]: caseValue });
                  }
                } else {
                  handleValueChange(null);
                }
              }}
            >
              <SelectTrigger>
                <SelectValue placeholder="Select variant..." />
              </SelectTrigger>
              <SelectContent>
                {typeDef.cases?.map(caseItem => {
                  const caseName =
                    typeof caseItem === "string" ? caseItem : caseItem.name;
                  return (
                    <SelectItem key={caseName} value={caseName}>
                      {caseName}
                    </SelectItem>
                  );
                })}
              </SelectContent>
            </Select>
            {(() => {
              if (!value || typeof value !== "object" || value === null)
                return null;

              // Handle the case where value is a variant record like { caseName: caseValue }
              const entries = Object.entries(value as Record<string, unknown>);
              if (entries.length === 1) {
                const entry = entries[0];
                if (!entry) return null;
                const [caseKey, caseValue] = entry;
                const selectedCase = typeDef.cases?.find(
                  c => (typeof c === "string" ? c : c.name) === caseKey,
                );

                if (selectedCase && typeof selectedCase !== "string") {
                  return (
                    <div className="pl-4 border-l-2 border-border/20">
                      <RecursiveParameterInput
                        key={caseKey}
                        name={caseKey}
                        typeDef={selectedCase.typ}
                        value={caseValue}
                        onChange={(_, newValue) => {
                          handleValueChange({ [caseKey]: newValue });
                        }}
                        path={currentPath}
                      />
                    </div>
                  );
                }
              }
              return null;
            })()}
          </div>
        );

      case "list":
        return (
          <div className="space-y-2">
            {Array.isArray(value) && value.length > 0 ? (
              <div className="space-y-2">
                {value.map((item, index) => (
                  <div key={index} className="flex gap-2 items-start">
                    <div className="flex-1">
                      <RecursiveParameterInput
                        name={index.toString()}
                        typeDef={typeDef.inner!}
                        value={item}
                        onChange={(_, newValue) => {
                          const newArray = [...((value as unknown[]) || [])];
                          newArray[index] = newValue;
                          handleValueChange(newArray);
                        }}
                        path={currentPath}
                      />
                    </div>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        const newArray = (value as unknown[]).filter(
                          (_, i) => i !== index,
                        );
                        handleValueChange(newArray);
                      }}
                      className="p-2 text-destructive hover:text-destructive/80"
                    >
                      <MinusCircle size={16} />
                    </Button>
                  </div>
                ))}
              </div>
            ) : (
              <div className="text-center py-2 text-muted-foreground text-sm">
                No items added
              </div>
            )}
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => {
                const newItem = createEmptyValue(typeDef.inner!);
                handleValueChange([...((value as unknown[]) || []), newItem]);
              }}
              className="flex items-center gap-1 text-primary hover:text-primary/80"
            >
              <PlusCircle size={16} />
              Add Item
            </Button>
          </div>
        );

      case "option":
        return (
          <div className="space-y-2">
            <div className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={value !== null && value !== undefined}
                onChange={e =>
                  handleValueChange(
                    e.target.checked ? createEmptyValue(typeDef.inner!) : null,
                  )
                }
                className="rounded border-border/20"
              />
              <span className="text-sm text-muted-foreground">
                Optional value
              </span>
            </div>
            {value !== null && value !== undefined && (
              <RecursiveParameterInput
                name={name}
                typeDef={typeDef.inner!}
                value={value}
                onChange={(_, newValue) => handleValueChange(newValue)}
                path={currentPath}
              />
            )}
          </div>
        );

      case "str":
      case "chr":
        return (
          <Input
            type="text"
            placeholder={`Enter ${name}...`}
            value={(value as string) || ""}
            onChange={e => handleValueChange(e.target.value)}
          />
        );

      case "bool":
        return (
          <RadioGroup
            value={String(value)}
            onValueChange={checked => handleValueChange(checked === "true")}
          >
            <div className="flex items-center space-x-2">
              <RadioGroupItem value="true" id={`${currentPath}-true`} />
              <Label htmlFor={`${currentPath}-true`}>True</Label>
            </div>
            <div className="flex items-center space-x-2">
              <RadioGroupItem value="false" id={`${currentPath}-false`} />
              <Label htmlFor={`${currentPath}-false`}>False</Label>
            </div>
          </RadioGroup>
        );

      case "enum":
        return (
          <Select
            value={(value as string) || ""}
            onValueChange={selectedValue => handleValueChange(selectedValue)}
          >
            <SelectTrigger>
              <SelectValue placeholder="Select an option" />
            </SelectTrigger>
            <SelectContent>
              {(typeDef.cases || []).map(option => {
                const optionName =
                  typeof option === "string" ? option : option.name;
                return (
                  <SelectItem key={optionName} value={optionName}>
                    {optionName}
                  </SelectItem>
                );
              })}
            </SelectContent>
          </Select>
        );

      case "flags":
        return (
          <div className="space-y-2">
            <div className="text-sm text-muted-foreground">Select flags:</div>
            {(typeDef.names || []).map(flagName => (
              <div key={flagName} className="flex items-center space-x-2">
                <input
                  type="checkbox"
                  id={`${currentPath}-${flagName}`}
                  checked={Array.isArray(value) && value.includes(flagName)}
                  onChange={e => {
                    const currentFlags = Array.isArray(value) ? value : [];
                    if (e.target.checked) {
                      handleValueChange([...currentFlags, flagName]);
                    } else {
                      handleValueChange(
                        currentFlags.filter(f => f !== flagName),
                      );
                    }
                  }}
                  className="rounded border-border/20"
                />
                <Label
                  htmlFor={`${currentPath}-${flagName}`}
                  className="text-sm"
                >
                  {flagName}
                </Label>
              </div>
            ))}
          </div>
        );

      case "result":
        return (
          <div className="space-y-4">
            <div className="flex items-center gap-4">
              <Label className="text-sm font-medium">Result type:</Label>
              <RadioGroup
                value={
                  value &&
                  typeof value === "object" &&
                  value !== null &&
                  "ok" in value
                    ? "ok"
                    : "err"
                }
                onValueChange={resultType => {
                  if (resultType === "ok") {
                    handleValueChange({
                      ok: typeDef.ok ? createEmptyValue(typeDef.ok) : null,
                    });
                  } else {
                    handleValueChange({
                      err: typeDef.err ? createEmptyValue(typeDef.err) : null,
                    });
                  }
                }}
              >
                <div className="flex items-center space-x-2">
                  <RadioGroupItem value="ok" id={`${currentPath}-ok`} />
                  <Label htmlFor={`${currentPath}-ok`}>Ok</Label>
                </div>
                <div className="flex items-center space-x-2">
                  <RadioGroupItem value="err" id={`${currentPath}-err`} />
                  <Label htmlFor={`${currentPath}-err`}>Error</Label>
                </div>
              </RadioGroup>
            </div>
            {(() => {
              if (!value || typeof value !== "object" || value === null)
                return null;

              const resultValue = value as Record<string, unknown>;

              return (
                <div className="pl-4 border-l-2 border-border/20">
                  {"ok" in resultValue && typeDef.ok && (
                    <RecursiveParameterInput
                      name="ok"
                      typeDef={typeDef.ok}
                      value={resultValue.ok}
                      onChange={(_, newValue) =>
                        handleValueChange({ ok: newValue })
                      }
                      path={currentPath}
                    />
                  )}
                  {"err" in resultValue && typeDef.err && (
                    <RecursiveParameterInput
                      name="err"
                      typeDef={typeDef.err}
                      value={resultValue.err}
                      onChange={(_, newValue) =>
                        handleValueChange({ err: newValue })
                      }
                      path={currentPath}
                    />
                  )}
                </div>
              );
            })()}
          </div>
        );

      case "tuple":
        return (
          <div className="space-y-2">
            <div className="text-sm text-muted-foreground">Tuple elements:</div>
            {typeDef.fields?.map((field, index) => (
              <div key={index} className="border-l-2 border-border/20 pl-4">
                <RecursiveParameterInput
                  name={`element-${index}`}
                  typeDef={field.typ}
                  value={Array.isArray(value) ? value[index] : undefined}
                  onChange={(_, newValue) => {
                    const newTuple = Array.isArray(value)
                      ? [...value]
                      : new Array(typeDef.fields?.length || 0);
                    newTuple[index] = newValue;
                    handleValueChange(newTuple);
                  }}
                  path={currentPath}
                />
              </div>
            ))}
          </div>
        );

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
        return (
          <Input
            type="number"
            placeholder={`Enter ${name}...`}
            value={(value as number) || ""}
            onChange={e => handleValueChange(Number(e.target.value))}
            step={typeStr.startsWith("f") ? "0.01" : "1"}
            min={typeStr.startsWith("u") ? "0" : undefined}
          />
        );

      default:
        return (
          <Textarea
            placeholder={`Enter ${name} (JSON format)...`}
            value={JSON.stringify(value, null, 2)}
            onChange={e => {
              try {
                const parsed = JSON.parse(e.target.value);
                handleValueChange(parsed);
              } catch {
                // Invalid JSON, keep as string for now
              }
            }}
            className="min-h-[100px] font-mono text-sm"
          />
        );
    }
  };

  return (
    <div className="space-y-2">
      <Label className="flex items-center gap-2 text-sm font-medium">
        {name}
        <TypeBadge type={typeDef.type} />
      </Label>
      {renderInput()}
    </div>
  );
};
