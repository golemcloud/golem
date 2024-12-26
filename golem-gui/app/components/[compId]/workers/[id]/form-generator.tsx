import React, { useId, useMemo } from "react";
import { useForm, Controller, Control, FieldErrors } from "react-hook-form";
import {
  TextField,
  Typography,
  Button,
  Stack,
  Box,
  FormControlLabel,
  Checkbox,
  Select,
  MenuItem,
} from "@mui/material";
import { Parameter } from "@/types/api";
import { AnalysedType_TypeVariant } from "@/types/golem-data-types";
import { getFormErrorMessage } from "@/lib/utils";

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
      // case "Tuple":
      //   defaults[field.name] = [generateDefaultValues(field.typ.items || [])];
      //   break;

      case "List":
        defaults[field.name] = [];
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
  errors: FieldErrors<FormData>
) => {
  const finalRootKey = `${
    rootKey
      ? `${rootKey}${!parameter?.ignoreDotConcat && parameter?.name ? "." : ""}`
      : ""
  }${parameter?.name || ""}`;
  // TODO need to add other types

  const paramType = parameter?.typ?.type || "";
  // TODO: Pending data types that needs work.
  //   | AnalysedType_TypeResult
  //   | AnalysedType_TypeOption
  //   | AnalysedType_TypeEnum
  //   | AnalysedType_TypeFlags
  //   | AnalysedType_TypeTuple
  //   | AnalysedType_TypeHandle;

  switch (true) {
    case ["Str", "S8", "S32", "Chr", "S64", "S16"].includes(paramType):
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            control={control}
            rules={{ required: `${parameter.name} is Mandatory` }}
            render={({ field: _field }) => (
              <TextField
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
                label={parameter.name}
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
          <Controller
            key={index}
            name={finalRootKey}
            rules={{ required: `${parameter.name} is Mandatory` }}
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
          <Controller
            key={index}
            name={finalRootKey}
            rules={{ required: `${parameter.name} is Mandatory` }}
            control={control}
            render={({}) => {
              const fields =
                parameter?.typ?.type == "Record"
                  ? parameter?.typ?.fields || []
                  : [];
              return (
                <div key={`${finalRootKey}`}>
                  <Typography variant="h6">{parameter.name}</Typography>
                  {fields?.map(
                    (nestedField: Parameter, nestedIndex: number) => (
                      <Box key={`${finalRootKey}_${nestedField.name}`}>
                        {generateField(
                          nestedField,
                          nestedIndex,
                          finalRootKey,
                          control,
                          handleChange,
                          errors
                        )}
                      </Box>
                    )
                  )}
                </div>
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
                      errors
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
                <>
                  <Stack
                    direction="row"
                    justifyContent={"space-between"}
                    alignItems={"center"}
                  >
                    <Typography>{parameter?.name}</Typography>
                    <Button
                      type="button"
                      onClick={(e) => {
                        e.preventDefault();
                        const newTuples = [...listValues, ""];
                        handleChange(finalRootKey, newTuples);
                      }}
                    >
                      Add Tuple
                    </Button>
                  </Stack>
                  {listValues?.map((_value: unknown, idx: number) => (
                    <fieldset key={`${finalRootKey}__${idx}`}>
                      <legend>
                        {_field.name} {idx}
                      </legend>

                      {/* Button to Remove Tuple */}
                      <Button
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
                        Remove Tuple
                      </Button>
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
                          errors
                        )}
                      </>
                    </fieldset>
                  ))}
                </>
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
  const defaultValues = useMemo(() => {
    return generateDefaultValues(config);
  }, [config]);

  const {
    control,
    handleSubmit,
    setValue,
    formState: { errors },
  } = useForm<FormData>({
    defaultValues: defaultValues,
  });

  console.log("defaultValues", defaultValues);

  const handleChange = (key: string, value: unknown) => {
    setValue(key, value, { shouldDirty: true });
  };

  return (
    <form onSubmit={handleSubmit(onSubmit)}>
      <Stack>
        {config.map((field, index) => (
          <Box key={`${id}__${field.name}__${index}`}>
            {generateField(field, index, "", control, handleChange, errors)}
          </Box>
        ))}
      </Stack>
      <Button
        type="submit"
        variant="contained"
        color="primary"
        className="mt-2"
      >
        Submit
      </Button>
    </form>
  );
};

export default DynamicForm;
