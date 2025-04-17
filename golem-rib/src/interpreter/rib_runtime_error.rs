use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
use crate::{CoercedNumericValue, InstructionId, TypeHint};
use golem_wasm_rpc::{Value, ValueAndType};
use std::fmt::Display;

#[derive(Debug, PartialEq, Clone)]
pub enum RibRuntimeError {
    InputNotFound(String),
    ExhaustedIterator,
    FieldNotFound {
        input: String, // string representing the stack value
        field: String,
    },
    InvariantViolation(InvariantViolation),
    ThrownError(String),
    CastError {
        from: CastFrom,
        to: TypeHint,
    },
    InvalidType {
        expected: Vec<TypeHint>,
        found: InvalidItem,
    },
    NoResult,
    InfiniteComputation {
        message: String,
    },
    IndexOutOfBounds {
        index: usize,
        size: usize,
    },
    InvalidComparison {
        message: String,
        left: Option<ValueAndType>,
        right: Option<ValueAndType>,
    },
    ArithmeticError {
        message: String,
        left: Option<CoercedNumericValue>,
        right: Option<CoercedNumericValue>,
    },
    FunctionInvokeError {
        function_name: String,
        error: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum CastFrom {
    FromValue(Value),
    FromType(TypeHint),
    FromCustom(String),
}

#[derive(Debug, Clone, PartialEq)]
enum InvalidItem {
    RuntimeValue(Value),
    Type(TypeHint),
    Custom(String),
}

pub fn arithmetic_error(
    message: &str,
    left: Option<&CoercedNumericValue>,
    right: Option<&CoercedNumericValue>,
) -> RibRuntimeError {
    RibRuntimeError::ArithmeticError {
        message: message.to_string(),
        left: left.cloned(),
        right: right.cloned(),
    }
}

pub fn cast_error(from: Value, to: TypeHint) -> RibRuntimeError {
    RibRuntimeError::CastError {
        from: CastFrom::FromValue(from),
        to,
    }
}

pub fn cast_error_custom<T: Display>(from: T, to: TypeHint) -> RibRuntimeError {
    RibRuntimeError::CastError {
        from: CastFrom::FromCustom(from.to_string()),
        to,
    }
}

pub fn empty_stack() -> RibRuntimeError {
    RibRuntimeError::InvariantViolation(InvariantViolation::InsufficientStackItems(1))
}

pub fn exhausted_iterator() -> RibRuntimeError {
    RibRuntimeError::ExhaustedIterator
}

pub fn field_not_found(input: String, field_name: &str) -> RibRuntimeError {
    RibRuntimeError::FieldNotFound {
        input,
        field: field_name.to_string(),
    }
}

pub fn function_invoke_fail(
    function_name: &str,
    error: Box<dyn std::error::Error + Send + Sync>,
) -> RibRuntimeError {
    RibRuntimeError::FunctionInvokeError {
        function_name: function_name.to_string(),
        error,
    }
}

pub fn index_out_of_bound(index: usize, size: usize) -> RibRuntimeError {
    RibRuntimeError::IndexOutOfBounds { index, size }
}

pub fn infinite_computation(message: &str) -> RibRuntimeError {
    RibRuntimeError::InfiniteComputation {
        message: message.to_string(),
    }
}

pub fn input_not_found(input_name: &str) -> RibRuntimeError {
    RibRuntimeError::InputNotFound(input_name.to_string())
}

pub fn instruction_jump_error(instruction_id: InstructionId) -> RibRuntimeError {
    RibRuntimeError::InvariantViolation(InvariantViolation::InstructionJumpError(instruction_id))
}

pub fn insufficient_stack_items(size: usize) -> RibRuntimeError {
    RibRuntimeError::InvariantViolation(InvariantViolation::InsufficientStackItems(1))
}

pub fn invalid_comparison(
    message: &str,
    left: Option<ValueAndType>,
    right: Option<ValueAndType>,
) -> RibRuntimeError {
    RibRuntimeError::InvalidComparison {
        message: message.to_string(),
        left,
        right,
    }
}

pub fn invalid_type_with_stack_value(
    expected: Vec<TypeHint>,
    found: RibInterpreterStackValue,
) -> RibRuntimeError {
    RibRuntimeError::InvalidType {
        expected,
        found: InvalidItem::Custom(found.to_string()),
    }
}

pub fn invalid_type_with_value(expected: Vec<TypeHint>, found: Value) -> RibRuntimeError {
    RibRuntimeError::InvalidType {
        expected,
        found: InvalidItem::RuntimeValue(found),
    }
}

pub fn invalid_value_with_type_hint(expected: Vec<TypeHint>, found: TypeHint) -> RibRuntimeError {
    RibRuntimeError::InvalidType {
        expected,
        found: InvalidItem::Type(found),
    }
}

pub fn no_result() -> RibRuntimeError {
    RibRuntimeError::NoResult
}

pub fn throw_error(message: &str) -> RibRuntimeError {
    RibRuntimeError::ThrownError(message.to_string())
}

#[macro_export]
macro_rules! corrupted_state {
    // This handles the case where no arguments are passed after the format string
    ($fmt:expr) => {{
        // Just return the error with the provided string
        RibRuntimeError::InvariantViolation(InvariantViolation::CorruptedState($fmt.to_string()))
    }};

    // This handles the case where arguments are passed
    ($fmt:expr, $($arg:tt)*) => {{
        // Create the error variant with the formatted message
        RibRuntimeError::InvariantViolation(InvariantViolation::CorruptedState(format!($fmt, $($arg)*)))
    }};
}

#[macro_export]
macro_rules! bail_corrupted_state {
    // This handles the case where no arguments are passed after the format string
    ($fmt:expr) => {{
        return Err(RibRuntimeError::InvariantViolation(InvariantViolation::CorruptedState($fmt.to_string())));
    }};

    // This handles the case where there are additional arguments
    ($fmt:expr, $($arg:tt)*) => {{
        return Err(RibRuntimeError::InvariantViolation(InvariantViolation::CorruptedState(format!($fmt, $($arg)*))));
    }};
}

#[derive(Debug, Clone, PartialEq)]
pub enum InvariantViolation {
    InsufficientStackItems(usize),
    CorruptedState(String),
    InstructionJumpError(InstructionId),
}
