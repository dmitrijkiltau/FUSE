#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamSpec<'a> {
    pub name: &'a str,
    pub has_default: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallArgSpec<'a> {
    pub name: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamBinding {
    Arg(usize),
    Default,
    MissingRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallBindError {
    UnknownArgument(String),
    DuplicateArgument(String),
    TooManyArguments,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallBindPlan {
    pub arg_to_param: Vec<Option<usize>>,
    pub param_bindings: Vec<ParamBinding>,
}

pub fn bind_call_args(
    params: &[ParamSpec<'_>],
    args: &[CallArgSpec<'_>],
) -> (CallBindPlan, Vec<CallBindError>) {
    let param_count = params.len();
    let mut errors = Vec::new();
    let mut assigned = vec![false; param_count];
    let mut param_to_arg: Vec<Option<usize>> = vec![None; param_count];
    let mut arg_to_param = Vec::with_capacity(args.len());
    let mut next_pos = 0usize;

    for (arg_idx, arg) in args.iter().enumerate() {
        let idx = if let Some(arg_name) = arg.name {
            params.iter().position(|param| param.name == arg_name)
        } else {
            while next_pos < param_count && assigned[next_pos] {
                next_pos += 1;
            }
            if next_pos < param_count {
                let idx = next_pos;
                next_pos += 1;
                Some(idx)
            } else {
                None
            }
        };

        match idx {
            Some(idx) if !assigned[idx] => {
                assigned[idx] = true;
                param_to_arg[idx] = Some(arg_idx);
                arg_to_param.push(Some(idx));
            }
            Some(idx) => {
                errors.push(CallBindError::DuplicateArgument(
                    params[idx].name.to_string(),
                ));
                arg_to_param.push(None);
            }
            None => {
                if let Some(arg_name) = arg.name {
                    errors.push(CallBindError::UnknownArgument(arg_name.to_string()));
                } else {
                    errors.push(CallBindError::TooManyArguments);
                }
                arg_to_param.push(None);
            }
        }
    }

    let mut param_bindings = Vec::with_capacity(param_count);
    for (idx, param) in params.iter().enumerate() {
        if let Some(arg_idx) = param_to_arg[idx] {
            param_bindings.push(ParamBinding::Arg(arg_idx));
        } else if param.has_default {
            param_bindings.push(ParamBinding::Default);
        } else {
            param_bindings.push(ParamBinding::MissingRequired);
        }
    }

    (
        CallBindPlan {
            arg_to_param,
            param_bindings,
        },
        errors,
    )
}

pub fn bind_positional_args(
    params: &[ParamSpec<'_>],
    provided_count: usize,
) -> (CallBindPlan, Vec<CallBindError>) {
    let args = vec![CallArgSpec { name: None }; provided_count];
    bind_call_args(params, &args)
}

#[cfg(test)]
mod tests {
    use super::{
        CallArgSpec, CallBindError, ParamBinding, ParamSpec, bind_call_args, bind_positional_args,
    };

    #[test]
    fn positional_binding_uses_defaults_and_marks_missing() {
        let params = [
            ParamSpec {
                name: "a",
                has_default: false,
            },
            ParamSpec {
                name: "b",
                has_default: true,
            },
            ParamSpec {
                name: "c",
                has_default: false,
            },
        ];
        let (plan, errors) = bind_positional_args(&params, 1);
        assert!(errors.is_empty());
        assert_eq!(
            plan.param_bindings,
            vec![
                ParamBinding::Arg(0),
                ParamBinding::Default,
                ParamBinding::MissingRequired
            ]
        );
    }

    #[test]
    fn named_and_positional_bindings_share_one_plan() {
        let params = [
            ParamSpec {
                name: "a",
                has_default: false,
            },
            ParamSpec {
                name: "b",
                has_default: false,
            },
            ParamSpec {
                name: "c",
                has_default: true,
            },
        ];
        let args = [CallArgSpec { name: None }, CallArgSpec { name: Some("c") }];
        let (plan, errors) = bind_call_args(&params, &args);
        assert!(errors.is_empty());
        assert_eq!(
            plan.param_bindings,
            vec![
                ParamBinding::Arg(0),
                ParamBinding::MissingRequired,
                ParamBinding::Arg(1)
            ]
        );
    }

    #[test]
    fn duplicate_and_unknown_named_arguments_are_reported() {
        let params = [ParamSpec {
            name: "a",
            has_default: false,
        }];
        let args = [
            CallArgSpec { name: Some("a") },
            CallArgSpec { name: Some("a") },
            CallArgSpec { name: Some("x") },
        ];
        let (_plan, errors) = bind_call_args(&params, &args);
        assert_eq!(
            errors,
            vec![
                CallBindError::DuplicateArgument("a".to_string()),
                CallBindError::UnknownArgument("x".to_string())
            ]
        );
    }

    #[test]
    fn too_many_positional_arguments_are_reported() {
        let params = [ParamSpec {
            name: "a",
            has_default: false,
        }];
        let (plan, errors) = bind_positional_args(&params, 2);
        assert_eq!(plan.arg_to_param, vec![Some(0), None]);
        assert_eq!(errors, vec![CallBindError::TooManyArguments]);
    }
}
