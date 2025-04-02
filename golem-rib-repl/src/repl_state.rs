use crate::dependency_manager::ComponentDependency;
use rib::{
    Interpreter, InterpreterEnv, InterpreterStack, RibByteCode, RibFunctionInvoke, RibInput,
};
use std::sync::Arc;

pub struct ReplState {
    byte_code: RibByteCode,
    interpreter: Interpreter,
    dependency: ComponentDependency,
    current_session_rib_texts: Vec<String>,
}

impl ReplState {
    pub fn current_rib_program(&self) -> String {
        self.current_session_rib_texts.join(";")
    }

    pub fn update_rib_text(&mut self, rib_text: &str) {
        self.current_session_rib_texts.push(rib_text.to_string());
    }

    pub fn pop_rib_text(&mut self) {
        self.current_session_rib_texts.pop();
    }

    pub fn interpreter(&mut self) -> &mut Interpreter {
        &mut self.interpreter
    }
    pub fn byte_code(&self) -> &RibByteCode {
        &self.byte_code
    }

    pub fn update_byte_code(&mut self, byte_code: RibByteCode) {
        self.byte_code = byte_code;
    }

    pub fn dependency(&self) -> &ComponentDependency {
        &self.dependency
    }

    pub fn new(
        dependency: &ComponentDependency,
        invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
    ) -> Self {
        let interpreter_env = InterpreterEnv::from(&RibInput::default(), &invoke);

        Self {
            byte_code: RibByteCode::default(),
            interpreter: Interpreter::new(
                &RibInput::default(),
                invoke,
                Some(InterpreterStack::default()),
                Some(interpreter_env),
            ),
            dependency: dependency.clone(),
            current_session_rib_texts: vec![],
        }
    }
}
