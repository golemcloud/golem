use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
use crate::{InstructionId, TypeHint};
use golem_wasm_rpc::{Value, ValueAndType};
use std::fmt::Display;

#[derive(Debug)]
pub enum RibRuntimeError {
    InputNotFound(String),
    ExhaustedIterator,
    FieldNotFound {
        field: String,
    },
    InvariantViolation(InvariantViolation),
    ThrownError(String),
    CastError {
        from: CastFrom,
        to: TypeHint,
    },
    TypeMismatch {
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
    },
    FunctionInvokeError {
        function_name: String,
        error: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum CastFrom {
    FromValue(Value),
    FromType(TypeHint),
    FromCustom(String),
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
    CorruptedState(String),
    InstructionJumpError(InstructionId),
}
