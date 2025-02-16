use crate::type_parameter::TypeParameter;
use crate::{DynamicParsedFunctionName, Expr, InferredType, TypeName};
use std::collections::VecDeque;
use crate::call_type::CallType;
use crate::instance_type::{FunctionName, InstanceType};

// This phase is responsible for identifying the worker function invocations
// worker.foo("x, y, z")
// Ensure worker is of the type `InstanceType`. This could be a resource of a component as well.
// The calls will be converted back to Expr::Call() itself making use of the `InstanceType` of LHS
// If `foo` is found to be a resource constructor, Expr::Invoke will be kept as is, as we make
// Invoke is lazy and the call is strict.
pub fn infer_worker_function_invokes(expr: &mut Expr) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        if let Expr::InvokeLazy {
            lhs,
            function_name,
            generic_type_parameter,
            args,
            inferred_type
        } = expr
        {
            // This should be an instance type if instance_type_binding phase has been run.
            // let m = instance("my-worker")
            // m.cart("x, y, z") // m is o the type instance-type
            let inferred_type = lhs.clone().inferred_type();

            match inferred_type {
                InferredType::Instance { instance_type } => {
                    let type_parameter = generic_type_parameter
                        .clone()
                        .map(|gtp| TypeParameter::from_str(&gtp.value))
                        .transpose()?;

                    // If the function is of the type resource then we need to update the expr to
                    // resource type
                    let function =
                        instance_type.get_function(function_name, type_parameter)?;

                    // if the function name is some sort of a resource constructor
                    // then  we update the inferred type of new Expr call to be a resource type
                    match function.function_name {
                        FunctionName::Function(function_name) => {
                            let dynamic_parsed_function_name = function_name.to_string();
                            let dynamic_parsed_function_name =
                                DynamicParsedFunctionName::parse(dynamic_parsed_function_name)?;

                            let new_call = Expr::call(dynamic_parsed_function_name, None, args.clone());
                            *expr = new_call;
                        }
                        // We are yet to be able to create a call_type
                        FunctionName::ResourceConstructor(fully_qualified_resource_constructor) => {
                            let new_call_type = CallType::ResourceConstruction {
                                resource_constructor: fully_qualified_resource_constructor,
                                resource_args: args.clone(),
                            };
                            // If this is a resource constructor
                            // then we make sure to have a new inferred type
                            let resource_instance_type = instance_type.get_function()
                            let new_call = Expr::Call(new_call_type, None, args.clone(), InferredType::Instance {
                                instance_type: InstanceType::Resource {
                                    worker_name: None,
                                    component_id: fully_qualified_resource_constructor.component_id().clone(),
                                },
                            });
                        }
                        // If this is resource method
                        FunctionName::ResourceMethod(resource_method) => {}
                    }

                    let new_call = Expr::call(function_name, None, args.clone());

                    *expr = new_call;
                }
                // This implies, none of the phase identified `lhs` to be an instance-type yet.
                // This would
                inferred_type => {
                    return Err(format!(
                        "Invalid worker function invoke. Expected {} to be an instance type, found {}",
                        lhs, TypeName::try_from(inferred_type).map(|x| x.to_string()).unwrap_or("Unknown".to_string())
                    ));
                }
            }
        }
        expr.visit_children_mut_bottom_up(&mut queue);
    }

    Ok(())
}


pub fn call_type(function: &FunctionName, resource_args: &mut Vec<Expr>, lhs_instance_type: InstanceType) -> Result<CallType, String> {
    match function {
        FunctionName::Function(fqn) => {
            let dynamic_parsed_function_name = fqn.to_string()?;
            DynamicParsedFunctionName::parse(fqn.to_string())?;
            Ok(CallType::Function(dynamic_parsed_function_name))
        }
        FunctionName::ResourceConstructor(resource_constructor) => {
            let new_instance_type = InstanceType::from(
                resource_constructor.component_id(),
                lhs_instance_type.function_type_registry.clone(),
                None,
            )?;

            Ok(CallType::ResourceConstruction {
                resource_args: resource_args.clone(),
                resource_constructor: resource_constructor.clone(),
            })
        }
        FunctionName::ResourceMethod(method_name) => {

        }
    }

    let name = self.function_name.to_string();
    DynamicParsedFunctionName::parse(name)
}