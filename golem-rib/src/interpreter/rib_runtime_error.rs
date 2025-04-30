use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
use crate::{InstructionId, TypeHint};
use golem_wasm_rpc::{Value, ValueAndType};
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum RibRuntimeError {
    ArithmeticError {
        message: String,
    },
    CastError {
        from: CastFrom,
        to: TypeHint,
    },
    ExhaustedIterator,
    FieldNotFound {
        field: String,
    },
    FunctionInvokeError {
        function_name: String,
        error: Box<dyn std::error::Error + Send + Sync>,
    },
    IndexOutOfBound {
        index: usize,
        size: usize,
    },
    InfiniteComputation {
        message: String,
    },
    InputNotFound(String),
    InvariantViolation(InvariantViolation),
    InvalidComparison {
        message: String,
        left: Option<ValueAndType>,
        right: Option<ValueAndType>,
    },
    NoResult,
    ThrownError(String),
    TypeMismatch {
        expected: Vec<TypeHint>,
        found: InvalidItem,
    },
}

impl std::error::Error for RibRuntimeError {}

#[derive(Debug, Clone, PartialEq)]
pub enum CastFrom {
    FromValue(Value),
    FromType(TypeHint),
    FromCustom(String),
}

impl Display for CastFrom {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CastFrom::FromValue(value) => write!(f, "{:?}", value),
            CastFrom::FromType(typ) => write!(f, "{}", typ),
            CastFrom::FromCustom(custom) => write!(f, "{}", custom),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InvalidItem {
    RuntimeValue(Value),
    Type(TypeHint),
    Custom(String),
}

pub fn arithmetic_error(message: &str) -> RibRuntimeError {
    RibRuntimeError::ArithmeticError {
        message: message.to_string(),
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

pub fn field_not_found(field_name: &str) -> RibRuntimeError {
    RibRuntimeError::FieldNotFound {
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
    RibRuntimeError::IndexOutOfBound { index, size }
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
    RibRuntimeError::InvariantViolation(InvariantViolation::InsufficientStackItems(size))
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
    RibRuntimeError::TypeMismatch {
        expected,
        found: InvalidItem::Custom(found.to_string()),
    }
}

pub fn type_mismatch_with_value(expected: Vec<TypeHint>, found: Value) -> RibRuntimeError {
    RibRuntimeError::TypeMismatch {
        expected,
        found: InvalidItem::RuntimeValue(found),
    }
}

pub fn type_mismatch_with_type_hint(expected: Vec<TypeHint>, found: TypeHint) -> RibRuntimeError {
    RibRuntimeError::TypeMismatch {
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

#[derive(Debug, Clone, PartialEq)]
pub enum InvariantViolation {
    InsufficientStackItems(usize),
    InternalCorruptedState(String),
    InstructionJumpError(InstructionId),
}

impl Display for RibRuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RibRuntimeError::InputNotFound(input_name) => {
                write!(f, "input not found: {}", input_name)
            }
            RibRuntimeError::ExhaustedIterator => write!(f, "no more values in iterator"),
            RibRuntimeError::FieldNotFound { field } => {
                write!(f, "field not found: {}", field)
            }
            RibRuntimeError::InvariantViolation(violation) => {
                write!(f, "internal error: {:?}", violation)
            }
            RibRuntimeError::ThrownError(message) => write!(f, "Thrown error: {}", message),
            RibRuntimeError::CastError { from, to } => {
                write!(f, "cast error from {} to {}", from, to)
            }
            RibRuntimeError::TypeMismatch { expected, found } => {
                write!(
                    f,
                    "runtime type mismatch: expected {:?}, found {:?}",
                    expected, found
                )
            }
            RibRuntimeError::NoResult => write!(f, "No result"),
            RibRuntimeError::InfiniteComputation { message } => {
                write!(f, "infinite computation detected: {}", message)
            }
            RibRuntimeError::IndexOutOfBound { index, size } => {
                write!(f, "index out of bound: {} (size: {})", index, size)
            }
            RibRuntimeError::InvalidComparison {
                message,
                left,
                right,
            } => match (left, right) {
                (Some(left), Some(right)) => {
                    write!(
                        f,
                        "Invalid comparison: {} (left: {}, right: {})",
                        message, left, right
                    )
                }
                _ => {
                    write!(f, "Invalid comparison: {} ", message)
                }
            },
            RibRuntimeError::ArithmeticError { message } => {
                write!(f, "arithmetic error: {}", message)
            }
            RibRuntimeError::FunctionInvokeError {
                function_name,
                error,
            } => {
                write!(f, "failed to invoke function {}: {}", function_name, error)
            }
        }
    }
}
