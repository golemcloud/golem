import React, { useEffect, useId, useMemo, useState } from "react";
import { useForm, Controller, Control, FieldErrors } from "react-hook-form";
import {
  TextField,
  Typography,
  Stack,
  Box,
  FormControlLabel,
  Checkbox,
  Select,
  MenuItem,
  Card,
  CardContent,
  Divider,
  Grid2 as Grid,
} from "@mui/material";
import { Parameter } from "@/types/api";
import {
  AnalysedType,
  AnalysedType_TypeVariant,
} from "@/types/golem-data-types";
import { getFormErrorMessage } from "@/lib/utils";
import { Button2 as Button } from "@/components/ui/button";
import {
  ChevronsLeftRight,
  Triangle,
  AlignVerticalSpaceAround,
} from "lucide-react";
import { transform } from "@/lib/hooks/use-worker";
import JsonEditor from "@/components/json-editor";
import { RemoveCircle } from "@mui/icons-material";

type FormData = {
  [key: string]: unknown;
};

const generateDefaultValues = (fields: Parameter[]): FormData => {
  const defaults: FormData = {};

  fields?.forEach((field) => {
    if (!field.name) {
      return;
    }
    switch (field?.typ?.type) {
      case "Record":
        defaults[field.name] = generateDefaultValues(field?.typ?.fields || []);
        break;
      case "Tuple":
      case "List":
        defaults[field.name] = [];
        break;
      case "Option":
        if (["List", "Tuple"].includes(field.typ?.inner?.type)) {
          defaults[field.name] = [];
        } else if (field.typ?.inner?.type == "Record") {
          defaults[field.name] = generateDefaultValues(
            field?.typ?.inner?.fields || []
          );
        } else {
          defaults[field.name] = "";
        }
        break;
      default:
        defaults[field.name] = "";
        break;
    }
  });

  return defaults;
};

const generateField = (
  parameter: Parameter & { ignoreDotConcat?: boolean },
  index: number,
  rootKey: string,
  control: Control<FormData>,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  handleChange: (key: string, value: any) => void,
  errors: FieldErrors<FormData>,
  optional?: boolean
) => {
  const finalRootKey = `${
    rootKey
      ? `${rootKey}${!parameter?.ignoreDotConcat && parameter?.name ? "." : ""}`
      : ""
  }${parameter?.name || ""}`;
  const paramType = parameter?.typ?.type || "";
  // TODO: Pending data types that needs work.
  //   | AnalysedType_TypeResult
  //   | AnalysedType_TypeEnum // done but not tested
  //   | AnalysedType_TypeFlags // done but not tested
  //   | AnalysedType_TypeHandle;

  switch (true) {
    case ["Str", "S8", "S32", "Chr", "S64", "S16"].includes(paramType):
      return (
        <>
          {/* <Typography>{parameter?.name}</Typography> */}
          <Controller
            key={index}
            name={finalRootKey}
            control={control}
            rules={
              optional ? {} : { required: `${parameter.name} is Mandatory` }
            }
            render={({ field: _field }) => (
              <TextField
                size="small"
                {..._field}
                label={parameter.name}
                variant="outlined"
                fullWidth
                placeholder={parameter.name}
                className="mt-2"
                // onChange={(e) => {
                //   handleChange(finalRootKey, e.target.value);
                // }}
              />
            )}
          />
          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
    case paramType == "Bool":
      return (
        <>
          <Typography>{parameter?.name}</Typography>
          <Controller
            key={index}
            name={finalRootKey}
            control={control}
            render={({ field: _field }) => (
              <FormControlLabel
                control={
                  <Checkbox
                    {..._field}
                    // onChange={(e) => {
                    //   handleChange(finalRootKey, e.target.checked);
                    // }}
                  />
                }
                label={""}
              />
            )}
          />
          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
    case ["F32", "F64", "U32", "U64", "U16", "U8"].includes(paramType):
      return (
        <>
          {/* <Typography>{parameter?.name}</Typography> */}
          <Controller
            key={index}
            name={finalRootKey}
            rules={
              optional ? {} : { required: `${parameter.name} is Mandatory` }
            }
            control={control}
            render={({ field: _field }) => (
              <TextField
                {..._field}
                label={parameter.name}
                type="number"
                variant="outlined"
                className="mt-2"
                fullWidth
                placeholder={parameter.name}
                onChange={(e) => {
                  handleChange(
                    finalRootKey,
                    e.target.value ? Number(e.target.value) : ""
                  );
                }}
              />
            )}
          />

          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
    case paramType === "Record":
      return (
        <>
          <Typography variant="h6">{parameter.name}</Typography>
          <Divider className="my-2 bg-border" />
          <Controller
            key={index}
            name={finalRootKey}
            rules={
              optional ? {} : { required: `${parameter.name} is Mandatory` }
            }
            control={control}
            render={({}) => {
              const fields =
                parameter?.typ?.type == "Record"
                  ? parameter?.typ?.fields || []
                  : [];
              return (
                <Box padding={1} key={`${finalRootKey}`}>
                  {fields?.map(
                    (nestedField: Parameter, nestedIndex: number) => (
                      <Box key={`${finalRootKey}_${nestedField.name}`}>
                        {generateField(
                          nestedField,
                          nestedIndex,
                          finalRootKey,
                          control,
                          handleChange,
                          errors,
                          optional
                        )}
                      </Box>
                    )
                  )}
                </Box>
              );
            }}
          />

          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
    case paramType === "Variant":
      const cases = (
        parameter?.typ && "cases" in parameter?.typ ? parameter?.typ?.cases : []
      ) as AnalysedType_TypeVariant["cases"];
      return (
        <>
          <Typography variant="h6">{parameter.name}</Typography>
          <Divider className="my-2 bg-border" />
          <Controller
            name={`${finalRootKey}`}
            rules={{ required: `${parameter?.name || 0} is Mandatory` }}
            control={control}
            render={({ field: { value: innerValue, ...innerField } }) => {
              const isEmpty =
                !innerValue || Object.keys(innerValue).length === 0;

              const selectValue = isEmpty
                ? 0
                : cases.findIndex(
                    (c: Parameter) =>
                      innerValue &&
                      c.name in (innerValue as Record<string, unknown>)
                  ) || 0;
              return (
                <>
                  <Select
                    {...innerField}
                    variant="outlined"
                    className="max-w-max"
                    value={selectValue}
                    onChange={(e) => {
                      const selectedIndex = Number(e.target.value);
                      if (selectedIndex < 0 || isNaN(selectedIndex)) {
                        return;
                      }
                      const selectedCase = cases[selectedIndex];
                      if (!selectedCase.typ) {
                        return alert("No fields found!");
                      }
                      const updatedValues = generateDefaultValues(
                        selectedCase ? [selectedCase] : []
                      );

                      handleChange(finalRootKey, updatedValues);
                    }}
                  >
                    {cases.map((_case: Parameter, in_idx: number) => (
                      <MenuItem
                        key={`${finalRootKey}_${_case.name}`}
                        value={in_idx}
                      >
                        {_case.name}
                      </MenuItem>
                    ))}
                  </Select>
                  <Box>
                    {generateField(
                      cases[selectValue],
                      selectValue,
                      `${finalRootKey}`,
                      control,
                      handleChange,
                      errors,
                      optional
                    )}
                  </Box>
                </>
              );
            }}
          />
          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
    case paramType === "List":
      return (
        <>
          <Controller
            name={finalRootKey}
            control={control}
            rules={{ required: `${parameter.name} is Mandatory` }}
            render={({ field: { value, ..._field } }) => {
              // due to type issue we are again check here. ideally this should not happen. or we need to use if case instead of switch
              const inner =
                parameter.typ?.type === "List"
                  ? parameter.typ.inner
                  : undefined;
              const listValues = (value || []) as Array<unknown>;
              return inner ? (
                <Box padding={1}>
                  <>
                    <Stack
                      direction="row"
                      justifyContent={"space-between"}
                      alignItems={"center"}
                    >
                      <Typography variant="h6">{parameter.name}</Typography>
                      <Button
                        variant="default"
                        size="sm"
                        type="button"
                        onClick={(e) => {
                          e.preventDefault();
                          const newTuples = [...listValues, ""];
                          handleChange(finalRootKey, newTuples);
                        }}
                      >
                        Add Element
                      </Button>
                    </Stack>

                    <Divider className="my-2 bg-border" />
                  </>
                  {listValues?.map((_value: unknown, idx: number) => (
                    <fieldset key={`${finalRootKey}__${idx}`}>
                      <Stack
                        direction={"row"}
                        justifyContent={"space-between"}
                        alignItems={"center"}
                      >
                        <legend>
                          {_field.name} {idx}
                        </legend>

                        {/* Button to Remove Tuple */}
                        <Button
                          variant="error"
                          size="sm"
                          type="button"
                          onClick={(e) => {
                            e.preventDefault();
                            const updatedTuples = [
                              ...listValues.slice(0, idx),
                              ...listValues.slice(idx + 1),
                            ];
                            handleChange(finalRootKey, updatedTuples);
                          }}
                        >
                          <RemoveCircle />
                        </Button>
                      </Stack>
                      <>
                        {generateField(
                          {
                            name: `[${idx}]`,
                            typ: inner,
                            // ignoreDotConcat: true,
                          },
                          idx,
                          `${finalRootKey}`,
                          control,
                          handleChange,
                          errors,
                          optional
                        )}
                      </>
                    </fieldset>
                  ))}
                </Box>
              ) : (
                <></>
              );
            }}
          />
          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );

    case paramType === "Option":
      return (
        <>
          <Controller
            name={finalRootKey}
            control={control}
            render={({}) => {
              const inner =
                parameter.typ?.type === "Option"
                  ? parameter.typ.inner
                  : undefined;
              return inner ? (
                <>
                  <Stack>
                    <Typography>{parameter?.name} (optional)</Typography>
                    {generateField(
                      {
                        name: "",
                        typ: inner,
                        // ignoreDotConcat: true,
                      },
                      index,
                      `${finalRootKey}`,
                      control,
                      handleChange,
                      errors,
                      true
                    )}
                  </Stack>
                </>
              ) : (
                <></>
              );
            }}
          />
        </>
      );

    case paramType === "Tuple":
      return (
        <>
          <Typography variant="h6">{parameter.name}</Typography>
          <Divider className="my-2 bg-border" />
          <Controller
            key={index}
            name={finalRootKey}
            rules={
              optional ? {} : { required: `${parameter.name} is Mandatory` }
            }
            control={control}
            render={({}) => {
              const items = (
                parameter?.typ?.type == "Tuple"
                  ? parameter?.typ?.items || []
                  : []
              ) as AnalysedType[];
              return (
                <div key={`${finalRootKey}`}>
                  <Typography variant="h6">{parameter.name}</Typography>
                  {items?.map((item: AnalysedType, nestedIndex: number) => {
                    return (
                      <Box key={`${finalRootKey}__${index}__${nestedIndex}`}>
                        {generateField(
                          {
                            name: `[${nestedIndex}]`,
                            typ: item,
                            // ignoreDotConcat: true,
                          },
                          index,
                          `${finalRootKey}`,
                          control,
                          handleChange,
                          errors,
                          optional
                        )}
                      </Box>
                    );
                  })}
                </div>
              );
            }}
          />

          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
      break;

    case ["Flags", "Emun"].includes(paramType):
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            rules={
              optional ? {} : { required: `${parameter.name} is Mandatory` }
            }
            control={control}
            render={({ field }) => {
              let options =
                parameter?.typ?.type === "Enum" ? parameter?.typ?.cases : [];
              options =
                parameter?.typ?.type === "Flags"
                  ? parameter?.typ?.names
                  : options;
              return (
                <>
                  <Typography>{finalRootKey}</Typography>
                  <Select
                    {...field}
                    variant="outlined"
                    className="max-w-max"
                    onChange={(e) => {
                      handleChange(finalRootKey, e.target.value);
                    }}
                  >
                    {options?.map((option: string, idx: number) => (
                      <MenuItem
                        key={`${finalRootKey}_${option}__${index}_${idx}`}
                        value={option}
                      >
                        {option}
                      </MenuItem>
                    ))}
                  </Select>
                </>
              );
            }}
          />
        </>
      );

    case paramType === null:
      return null;
    case paramType === undefined:
      return null;
    default:
      return <Typography>Some Data types were not configued.</Typography>;
  }
};

const DynamicForm: React.FC<{
  config: Parameter[];
  onSubmit: (data: FormData) => void;
}> = ({ config, onSubmit }) => {
  const id = useId();
  const [tab, setTab] = useState<number>(0);
  const defaultValues = useMemo(() => {
    return generateDefaultValues(config);
  }, [config]);

  const {
    control,
    handleSubmit,
    setValue,
    formState: { errors },
    getValues,
    reset,
  } = useForm<FormData>({
    defaultValues: {},
  });

  console.log("defaultValues", defaultValues);

  useEffect(() => {
    reset(defaultValues || {}); // Reset with fresh default values
    setTab(0); // Reset tab to the initial state
  }, [config, reset, defaultValues])

  const handleChange = (key: string, value: unknown) => {
    setValue(key, value, { shouldDirty: true });
  };

  return (
    <>
      <form onSubmit={handleSubmit(onSubmit)}>
        <Grid
          container
          spacing={3}
          sx={{ flexWrap: "wrap" }}
          className="bg-[#dedede] dark:bg-[#0a0a0a] p-2 px-2 mb-10"
        >
          <Grid size={{ xs: 12, sm: 6 }}>
            <Box className="flex sm:justify-start justify-between">
              <Button
                variant="dropdown"
                size="md"
                className="mx-2"
                onClick={(e) => {
                  e.preventDefault();
                  setTab(0);
                }}
              >
                Form (x)
              </Button>
              <Button
                variant="dropdown"
                size="md"
                className="mx-2"
                onClick={(e) => {
                  e.preventDefault();
                  setTab(1);
                }}
              >
                Preview <AlignVerticalSpaceAround />
              </Button>
            </Box>
          </Grid>
          <Grid size={{ xs: 12, sm: 6 }}>
            <Box className="flex sm:justify-end justify-between">
              <Button
                variant="secondary"
                size="md"
                className="mx-2"
                onClick={(e) => {
                  e.preventDefault();
                  setTab(2);
                }}
              >
                Types <ChevronsLeftRight />
              </Button>
              <Button
                variant="success"
                size="md"
                type="submit"
                className="mx-2"
              >
                Invoke <Triangle className="rotate-90" />
              </Button>
            </Box>
          </Grid>
        </Grid>
        <Stack>
          {tab === 0 &&
            config.map((field, index) => (
              <Box key={`${id}__${field.name}__${index}`}>
                {generateField(field, index, "", control, handleChange, errors)}
              </Box>
            ))}
          {tab === 1 && (
            <Card sx={{ backgroundColor: "#1e1e1e", color: "#fff" }}>
              <CardContent>
                <Typography variant="h6" gutterBottom>
                  Preview
                </Typography>
                <Box
                  component="pre"
                  sx={{
                    backgroundColor: "#121212",
                    padding: 2,
                    borderRadius: 1,
                    color: "#9cdcfe",
                    overflow: "auto",
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                  }}
                >
                  <JsonEditor json={config.length ? getValues(): {}} />
                </Box>
              </CardContent>
            </Card>
          )}
          {tab === 2 && (
            <Card sx={{ backgroundColor: "#1e1e1e", color: "#fff" }}>
              <CardContent>
                <Typography variant="h6" gutterBottom>
                  Preview
                </Typography>
                <Box
                  component="pre"
                  sx={{
                    backgroundColor: "#121212",
                    padding: 2,
                    borderRadius: 1,
                    color: "#9cdcfe",
                    overflow: "auto",
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                  }}
                >
                  <JsonEditor
                    json={{ params: transform(config, config.length ? getValues(): {}) }}
                  />
                </Box>
              </CardContent>
            </Card>
          )}
        </Stack>
      </form>
    </>
  );
};

export default DynamicForm;
