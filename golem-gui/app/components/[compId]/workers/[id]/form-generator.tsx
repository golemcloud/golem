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
import { Parameter, RecordTyp } from "@/types/api";
import { getFormErrorMessage } from "@/lib/utils";

type FormData = {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  [key: string]: any; // Using `any` here for dynamic keys
};

const generateDefaultValues = (fields: Parameter[]): FormData => {
  const defaults: FormData = {};

  fields?.forEach((field) => {
    if (!field.name) {
      return;
    }
    switch (field?.typ?.type) {
      case "Record":
        defaults[field.name] = generateDefaultValues(field.typ.fields || []);
        break;
      // case "Tuple":
      //   defaults[field.name] = [generateDefaultValues(field.typ.items || [])];
      //   break;

      case "List":
        //   defaults[field.name] = {options: (field.typ?.inner?.cases || []).map((_case)=>{
        //      return generateDefaultValues([_case])
        //   }),
        //   value: []
        // }
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
  field: Parameter,
  index: number,
  rootKey: string,
  control: Control<FormData>,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  handleChange: (key: string, value: any) => void,
  errors: FieldErrors<FormData>
) => {
  const finalRootKey = `${rootKey ? `${rootKey}.` : ""}${field.name}`;
  // TODO need to add other types

  switch (field?.typ?.type) {
    case "Str":
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            control={control}
            rules={{ required: `${field.name} is Mandatory` }}
            render={({ field: _field }) => (
              <TextField
                {..._field}
                label={field.name}
                variant="outlined"
                fullWidth
                placeholder={field.name}
                className="mt-2"
                onChange={(e) => {
                  handleChange(finalRootKey, e.target.value);
                }}
              />
            )}
          />
          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
    case "Bool":
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            control={control}
            rules={{ required: `${field.name} is Mandatory` }}
            render={({ field: _field }) => (
              <FormControlLabel
                control={
                  <Checkbox
                    {..._field}
                    checked={_field.value || false} // Ensure a boolean value
                    onChange={(e) => {
                      _field.onChange(e.target.checked);
                      handleChange(finalRootKey, e.target.checked);
                    }}
                  />
                }
                label={field.name}
              />
            )}
          />
          <Typography variant="caption" color="error">
            {getFormErrorMessage(finalRootKey, errors)}
          </Typography>
        </>
      );
    case "F32":
    case "F64":
    case "F16":
    case "U32":
    case "U64":
    case "U16":
    case "U8":
    case "F8":
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            rules={{ required: `${field.name} is Mandatory` }}
            control={control}
            render={({ field: _field }) => (
              <TextField
                {..._field}
                label={field.name}
                type="number"
                variant="outlined"
                className="mt-2"
                fullWidth
                placeholder={field.name}
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
    case "Record":
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            rules={{ required: `${field.name} is Mandatory` }}
            control={control}
            render={({}) => {
              const fields =
                field.typ.type == "Record" ? field?.typ?.fields || [] : [];
              return (
                <div key={`${finalRootKey}`}>
                  <Typography variant="h6">{field.name}</Typography>
                  {fields?.map((nestedField, nestedIndex) => (
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
                  ))}
                </div>
              );
            }}
          />
        </>
      );

    case "List":
      return (
        <>
          <Controller
            key={index}
            name={finalRootKey}
            control={control}
            render={({ field: { value, ..._field } }) => {
              const cases =
                field.typ.type === "List" ? field.typ.inner.cases || [] : [];
              return (
                <>
                <Stack direction="row" justifyContent={"space-between"} alignItems={"center"}>

                  <Typography>{field.name}</Typography>
                  <Button
                    type="button"
                    onClick={(e) => {
                      e.preventDefault();
                      const newTuples = [
                        ...value,
                        generateDefaultValues(cases[0] ? [cases[0]] : []),
                      ];
                      handleChange(finalRootKey, newTuples);
                    }}
                    >
                    Add Tuple
                  </Button>
                    </Stack>
                  {value?.map((_value: Record<string, any>, idx: number) => (
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
                            ...value.slice(0, idx), // Take all elements before the index
                            ...value.slice(idx + 1), // Take all elements after the index
                          ];
                          console.log("updatedTuples=====>", updatedTuples);

                          handleChange(finalRootKey, updatedTuples);
                        }}
                      >
                        Remove Tuple
                      </Button>

                      {/* Dropdown to Select Case */}
                      <Controller
                        name={`${finalRootKey}[${idx}]`}
                        control={control}
                        render={({
                          field: { value: innerValue, ...innerField },
                        }) => (
                          <>
                            <Select
                              {...innerField}
                              variant="outlined"
                              className="max-w-max"
                              value={
                                cases.findIndex(
                                  (c) => innerValue && c.name in innerValue
                                ) || 0
                              }
                              onChange={(e) => {
                                const selectedIndex = Number(e.target.value);
                                const selectedCase = cases[selectedIndex];
                                const updatedValues = generateDefaultValues(
                                  selectedCase ? [selectedCase] : []
                                );
                                handleChange(
                                  `${finalRootKey}[${idx}]`,
                                  updatedValues
                                );
                              }}
                            >
                              {cases.map((_case, index) => (
                                <MenuItem
                                  key={`${finalRootKey}[${idx}_${_case.name}_${index}`}
                                  value={index}
                                >
                                  {_case.name}
                                </MenuItem>
                              ))}
                            </Select>

                            {/* Render Fields for Selected Case */}
                            {innerValue &&
                              Object.entries(innerValue).map(
                                ([key, fieldValue], _idx) =>
                                  cases.map(
                                    (_case, index) =>
                                      _case.name === key && (
                                        <Box
                                          key={`${finalRootKey}[${idx}_${_case.name}_${_idx}`}
                                        >
                                          {generateField(
                                            _case,
                                            _idx,
                                            `${finalRootKey}[${idx}]`,
                                            control,
                                            handleChange,
                                            errors
                                          )}
                                        </Box>
                                      )
                                  )
                              )}
                          </>
                        )}
                      />
                    </fieldset>
                  ))}

                  {/* Button to Add Tuple */}
                  
                </>
              );
            }}
          />
        </>
      );
    case null: return null;
    case undefined: return null;
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

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const handleChange = (key: string, value: any) => {
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
