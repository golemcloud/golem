use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::Value;
use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
use crate::{InstructionId, TypeHint};

#[derive(Debug, Clone, PartialEq)]
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
        from: Value,
        to: TypeHint,
    },
    InvalidType {
        expected: Vec<TypeHint>,
        found: InvalidItem
    },
    NoResult,
    InfiniteComputation {
        message: String
    },
    IndexOutOfBounds {
        index: usize,
        size: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum InvalidItem {
    RuntimeValue(Value),
    Type(TypeHint),
    Custom(String),
}

pub fn throw_error(message: &str) -> RibRuntimeError {
    RibRuntimeError::ThrownError(message.to_string())
}

pub fn cast_error(from: Value, to: TypeHint) -> RibRuntimeError {
    RibRuntimeError::CastError {
        from,
        to
    }
}

pub fn no_result() -> RibRuntimeError {
    RibRuntimeError::NoResult
}

pub fn exhausted_iterator() -> RibRuntimeError {
    RibRuntimeError::ExhaustedIterator
}

pub fn input_not_found(input_name: &str) -> RibRuntimeError {
    RibRuntimeError::InputNotFound(input_name.to_string())
}

pub fn field_not_found(input: RibInterpreterStackValue, field_name: &str) -> RibRuntimeError {
    RibRuntimeError::FieldNotFound {
        input: input.to_string(),
        field: field_name.to_string(),
    }
}

pub fn invalid_type(expected: Vec<TypeHint>, found: Value) -> RibRuntimeError {
    RibRuntimeError::InvalidType {
        expected,
        found: InvalidItem::RuntimeValue(found),
    }
}
pub fn invalid_type_custom(expected: Vec<TypeHint>, found: RibInterpreterStackValue) -> RibRuntimeError {
    RibRuntimeError::InvalidType {
        expected,
        found: InvalidItem::Custom(found.to_string()),
    }
}


pub fn empty_stack() -> RibRuntimeError {
    RibRuntimeError::InvariantViolation(InvariantViolation::InsufficientStackItems(1))
}

pub fn instruction_jump_error(instruction_id: InstructionId) -> RibRuntimeError {
    RibRuntimeError::InvariantViolation(InvariantViolation::InstructionJumpError(instruction_id))
}

pub fn insufficient_stack_items(size: usize) -> RibRuntimeError {
    RibRuntimeError::InvariantViolation(InvariantViolation::InsufficientStackItems(1))
}

pub fn infinite_computation(message: &str) -> RibRuntimeError {
    RibRuntimeError::InfiniteComputation {
        message: message.to_string()
    }
}

pub fn index_out_of_bounds(index: usize, size: usize) -> RibRuntimeError {
    RibRuntimeError::IndexOutOfBounds {
        index,
        size
    }
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
    InstructionJumpError(InstructionId)
}