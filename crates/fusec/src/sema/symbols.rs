use std::collections::HashMap;

use crate::ast::{
    Block, ComponentDecl, ConfigDecl, EnumDecl, Expr, ExprKind, FnDecl, Ident, ImplDecl,
    ImportDecl, ImportSpec, InterfaceDecl, InterfaceMember, InterpPart, Item, Pattern,
    PatternKind, Program, ServiceDecl, Stmt, StmtKind, TypeDecl, TypeRef, TypeRefKind,
};
use crate::diag::Diagnostics;
use crate::loader::{ImportPathKind, classify_import_path};
use crate::span::Span;

#[derive(Clone, Debug, Default)]
pub struct ModuleSymbols {
    pub types: HashMap<String, TypeInfo>,
    pub enums: HashMap<String, EnumInfo>,
    pub interfaces: HashMap<String, InterfaceInfo>,
    pub functions: HashMap<String, FnSigRef>,
    pub configs: HashMap<String, ConfigInfo>,
    pub services: HashMap<String, ServiceInfo>,
    pub imports: HashMap<String, ImportInfo>,
    pub impls: Vec<ImplInfo>,
}

impl ModuleSymbols {
    pub fn import_kind(&self, name: &str) -> Option<&ImportKind> {
        self.imports.get(name).map(|info| &info.kind)
    }

    pub fn is_imported(&self, name: &str) -> bool {
        self.imports.contains_key(name)
    }
}

#[derive(Clone, Debug)]
pub struct TypeInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub name: String,
    pub ty: TypeRef,
    pub span: Span,
    pub has_default: bool,
}

#[derive(Clone, Debug)]
pub struct EnumInfo {
    pub name: String,
    pub variants: Vec<EnumVariantInfo>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumVariantInfo {
    pub name: String,
    pub payload: Vec<TypeRef>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct InterfaceInfo {
    pub name: String,
    pub members: Vec<InterfaceMemberInfo>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TypeParamRef {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct WhereConstraintRef {
    pub type_param: String,
    pub interface: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct InterfaceMemberInfo {
    pub name: String,
    pub type_params: Vec<TypeParamRef>,
    pub params: Vec<ParamRef>,
    pub ret: Option<TypeRef>,
    pub where_clause: Vec<WhereConstraintRef>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnSigRef {
    pub name: String,
    pub type_params: Vec<TypeParamRef>,
    pub params: Vec<ParamRef>,
    pub ret: Option<TypeRef>,
    pub where_clause: Vec<WhereConstraintRef>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ParamRef {
    pub name: String,
    pub ty: TypeRef,
    pub has_default: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ImplInfo {
    pub interface: String,
    pub target: String,
    pub methods: Vec<ImplMethodInfo>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ImplMethodInfo {
    pub name: String,
    pub type_params: Vec<TypeParamRef>,
    pub params: Vec<ParamRef>,
    pub ret: Option<TypeRef>,
    pub where_clause: Vec<WhereConstraintRef>,
    pub span: Span,
    pub uses_self: bool,
}

#[derive(Clone, Debug)]
pub struct ConfigInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ServiceInfo {
    pub name: String,
    pub routes: Vec<RouteSig>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct RouteSig {
    pub span: Span,
    pub body_type: Option<TypeRef>,
    pub ret_type: TypeRef,
}

#[derive(Clone, Debug)]
pub struct ImportInfo {
    pub name: String,
    pub kind: ImportKind,
    pub path: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ImportKind {
    Module,
    Item,
    AssetMarkdown,
    AssetJson,
}

pub fn collect(program: &Program, diags: &mut Diagnostics) -> ModuleSymbols {
    let mut types = HashMap::new();
    let mut enums = HashMap::new();
    let mut interfaces = HashMap::new();
    let mut functions = HashMap::new();
    let mut configs = HashMap::new();
    let mut services = HashMap::new();
    let mut imports = HashMap::new();
    let mut impls = Vec::new();
    let mut names: HashMap<String, Span> = HashMap::new();

    for item in &program.items {
        match item {
            Item::Import(decl) => collect_import(decl, &mut imports, &mut names, diags),
            Item::Type(decl) => collect_type(decl, &mut types, &mut names, diags),
            Item::Enum(decl) => collect_enum(decl, &mut enums, &mut names, diags),
            Item::Interface(decl) => collect_interface(decl, &mut interfaces, &mut names, diags),
            Item::Fn(decl) => collect_fn(decl, &mut functions, &mut names, diags),
            Item::Component(decl) => collect_component(decl, &mut functions, &mut names, diags),
            Item::Config(decl) => collect_config(decl, &mut configs, &mut names, diags),
            Item::Service(decl) => collect_service(decl, &mut services, &mut names, diags),
            Item::Impl(decl) => impls.push(collect_impl(decl)),
            Item::App(_) | Item::Migration(_) | Item::Test(_) => {}
        }
    }

    ModuleSymbols {
        types,
        enums,
        interfaces,
        functions,
        configs,
        services,
        imports,
        impls,
    }
}

fn register_name(
    name: &str,
    span: Span,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) -> bool {
    if let Some(prev) = names.get(name) {
        diags.error(span, format!("duplicate symbol: {name}"));
        diags.error(*prev, format!("previous definition of {name} here"));
        false
    } else {
        names.insert(name.to_string(), span);
        true
    }
}

fn collect_import(
    decl: &ImportDecl,
    imports: &mut HashMap<String, ImportInfo>,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) {
    match &decl.spec {
        ImportSpec::Module { name } => {
            if register_name(&name.name, name.span, names, diags) {
                imports.insert(
                    name.name.clone(),
                    ImportInfo {
                        name: name.name.clone(),
                        kind: ImportKind::Module,
                        path: None,
                        span: name.span,
                    },
                );
            }
        }
        ImportSpec::ModuleFrom { name, path } => {
            let kind = match classify_import_path(&path.value) {
                ImportPathKind::Asset(crate::loader::ImportedAssetKind::Markdown) => {
                    ImportKind::AssetMarkdown
                }
                ImportPathKind::Asset(crate::loader::ImportedAssetKind::Json) => {
                    ImportKind::AssetJson
                }
                _ => ImportKind::Module,
            };
            if register_name(&name.name, name.span, names, diags) {
                imports.insert(
                    name.name.clone(),
                    ImportInfo {
                        name: name.name.clone(),
                        kind,
                        path: Some(path.value.clone()),
                        span: name.span,
                    },
                );
            }
        }
        ImportSpec::NamedFrom {
            names: import_names,
            path,
        } => {
            for name in import_names {
                if register_name(&name.name, name.span, names, diags) {
                    imports.insert(
                        name.name.clone(),
                        ImportInfo {
                            name: name.name.clone(),
                            kind: ImportKind::Item,
                            path: Some(path.value.clone()),
                            span: name.span,
                        },
                    );
                }
            }
        }
        ImportSpec::AliasFrom {
            name: _name,
            alias,
            path,
        } => {
            if register_name(&alias.name, alias.span, names, diags) {
                imports.insert(
                    alias.name.clone(),
                    ImportInfo {
                        name: alias.name.clone(),
                        kind: ImportKind::Module,
                        path: Some(path.value.clone()),
                        span: alias.span,
                    },
                );
            }
        }
    }
}

fn collect_type(
    decl: &TypeDecl,
    types: &mut HashMap<String, TypeInfo>,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) {
    if !register_name(&decl.name.name, decl.name.span, names, diags) {
        return;
    }
    let fields = decl
        .fields
        .iter()
        .map(|field| FieldInfo {
            name: field.name.name.clone(),
            ty: field.ty.clone(),
            span: field.span,
            has_default: field.default.is_some(),
        })
        .collect();
    types.insert(
        decl.name.name.clone(),
        TypeInfo {
            name: decl.name.name.clone(),
            fields,
            span: decl.span,
        },
    );
}

fn collect_enum(
    decl: &EnumDecl,
    enums: &mut HashMap<String, EnumInfo>,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) {
    if !register_name(&decl.name.name, decl.name.span, names, diags) {
        return;
    }
    let variants = decl
        .variants
        .iter()
        .map(|variant| EnumVariantInfo {
            name: variant.name.name.clone(),
            payload: variant.payload.clone(),
            span: variant.span,
        })
        .collect();
    enums.insert(
        decl.name.name.clone(),
        EnumInfo {
            name: decl.name.name.clone(),
            variants,
            span: decl.span,
        },
    );
}

fn collect_interface(
    decl: &InterfaceDecl,
    interfaces: &mut HashMap<String, InterfaceInfo>,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) {
    if !register_name(&decl.name.name, decl.name.span, names, diags) {
        return;
    }
    let members = decl.members.iter().map(interface_member_info).collect();
    interfaces.insert(
        decl.name.name.clone(),
        InterfaceInfo {
            name: decl.name.name.clone(),
            members,
            span: decl.span,
        },
    );
}

fn interface_member_info(member: &InterfaceMember) -> InterfaceMemberInfo {
    InterfaceMemberInfo {
        name: member.name.name.clone(),
        type_params: type_params_ref(&member.type_params),
        params: params_ref(&member.params),
        ret: member.ret.clone(),
        where_clause: where_constraints_ref(&member.where_clause),
        span: member.span,
    }
}

fn collect_fn(
    decl: &FnDecl,
    functions: &mut HashMap<String, FnSigRef>,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) {
    if !register_name(&decl.name.name, decl.name.span, names, diags) {
        return;
    }
    functions.insert(decl.name.name.clone(), fn_sig_ref_from_fn_decl(decl));
}

fn fn_sig_ref_from_fn_decl(decl: &FnDecl) -> FnSigRef {
    FnSigRef {
        name: decl.name.name.clone(),
        type_params: type_params_ref(&decl.type_params),
        params: params_ref(&decl.params),
        ret: decl.ret.clone(),
        where_clause: where_constraints_ref(&decl.where_clause),
        span: decl.span,
    }
}

fn type_params_ref(type_params: &[crate::ast::TypeParam]) -> Vec<TypeParamRef> {
    type_params
        .iter()
        .map(|param| TypeParamRef {
            name: param.name.name.clone(),
            span: param.span,
        })
        .collect()
}

fn where_constraints_ref(constraints: &[crate::ast::WhereConstraint]) -> Vec<WhereConstraintRef> {
    constraints
        .iter()
        .map(|constraint| WhereConstraintRef {
            type_param: constraint.type_param.name.clone(),
            interface: constraint.interface.name.clone(),
            span: constraint.span,
        })
        .collect()
}

fn params_ref(params: &[crate::ast::Param]) -> Vec<ParamRef> {
    params
        .iter()
        .map(|param| ParamRef {
            name: param.name.name.clone(),
            ty: param.ty.clone(),
            has_default: param.default.is_some(),
            span: param.span,
        })
        .collect()
}

fn collect_impl(decl: &ImplDecl) -> ImplInfo {
    let methods = decl
        .methods
        .iter()
        .map(|method| ImplMethodInfo {
            name: method.name.name.clone(),
            type_params: type_params_ref(&method.type_params),
            params: params_ref(&method.params),
            ret: method.ret.clone(),
            where_clause: where_constraints_ref(&method.where_clause),
            span: method.span,
            uses_self: fn_decl_uses_ident(method, "self"),
        })
        .collect();
    ImplInfo {
        interface: decl.interface.name.clone(),
        target: decl.target.name.clone(),
        methods,
        span: decl.span,
    }
}

fn fn_decl_uses_ident(decl: &FnDecl, ident: &str) -> bool {
    decl.params
        .iter()
        .filter_map(|param| param.default.as_ref())
        .any(|expr| expr_uses_ident(expr, ident))
        || block_uses_ident(&decl.body, ident)
}

fn block_uses_ident(block: &Block, ident: &str) -> bool {
    block.stmts.iter().any(|stmt| stmt_uses_ident(stmt, ident))
}

fn stmt_uses_ident(stmt: &Stmt, ident: &str) -> bool {
    match &stmt.kind {
        StmtKind::Let { ty: _, expr, .. } | StmtKind::Var { ty: _, expr, .. } => {
            expr_uses_ident(expr, ident)
        }
        StmtKind::Assign { target, expr } => {
            expr_uses_ident(target, ident) || expr_uses_ident(expr, ident)
        }
        StmtKind::Return { expr } => expr.as_ref().is_some_and(|expr| expr_uses_ident(expr, ident)),
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            expr_uses_ident(cond, ident)
                || block_uses_ident(then_block, ident)
                || else_if
                    .iter()
                    .any(|(expr, block)| expr_uses_ident(expr, ident) || block_uses_ident(block, ident))
                || else_block
                    .as_ref()
                    .is_some_and(|block| block_uses_ident(block, ident))
        }
        StmtKind::Match { expr, cases } => {
            expr_uses_ident(expr, ident)
                || cases
                    .iter()
                    .any(|(pat, block)| pattern_uses_ident(pat, ident) || block_uses_ident(block, ident))
        }
        StmtKind::For { pat, iter, block } => {
            pattern_uses_ident(pat, ident) || expr_uses_ident(iter, ident) || block_uses_ident(block, ident)
        }
        StmtKind::While { cond, block } => expr_uses_ident(cond, ident) || block_uses_ident(block, ident),
        StmtKind::Transaction { block } => block_uses_ident(block, ident),
        StmtKind::Expr(expr) => expr_uses_ident(expr, ident),
        StmtKind::Break | StmtKind::Continue => false,
    }
}

fn expr_uses_ident(expr: &Expr, ident: &str) -> bool {
    match &expr.kind {
        ExprKind::Literal(_) => false,
        ExprKind::Ident(name) => name.name == ident,
        ExprKind::Binary { left, right, .. } | ExprKind::Coalesce { left, right } => {
            expr_uses_ident(left, ident) || expr_uses_ident(right, ident)
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Await { expr }
        | ExprKind::Box { expr }
        | ExprKind::BangChain { expr, error: None } => expr_uses_ident(expr, ident),
        ExprKind::BangChain {
            expr,
            error: Some(error),
        } => expr_uses_ident(expr, ident) || expr_uses_ident(error, ident),
        ExprKind::Call {
            callee,
            args,
            type_args: _,
        } => {
            expr_uses_ident(callee, ident)
                || args.iter().any(|arg| {
                    arg.name.as_ref().is_some_and(|name| name.name == ident)
                        || expr_uses_ident(&arg.value, ident)
                })
        }
        ExprKind::Member { base, name } | ExprKind::OptionalMember { base, name } => {
            name.name == ident || expr_uses_ident(base, ident)
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            expr_uses_ident(base, ident) || expr_uses_ident(index, ident)
        }
        ExprKind::StructLit { name, fields } => {
            name.name == ident || fields.iter().any(|field| expr_uses_ident(&field.value, ident))
        }
        ExprKind::ListLit(items) => items.iter().any(|item| expr_uses_ident(item, ident)),
        ExprKind::MapLit(items) => items
            .iter()
            .any(|(key, value)| expr_uses_ident(key, ident) || expr_uses_ident(value, ident)),
        ExprKind::InterpString(parts) => parts.iter().any(|part| match part {
            InterpPart::Text(_) => false,
            InterpPart::Expr(expr) => expr_uses_ident(expr, ident),
        }),
        ExprKind::Spawn { block } => block_uses_ident(block, ident),
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            expr_uses_ident(cond, ident)
                || then_children.iter().any(|expr| expr_uses_ident(expr, ident))
                || else_if.iter().any(|(cond, children)| {
                    expr_uses_ident(cond, ident)
                        || children.iter().any(|expr| expr_uses_ident(expr, ident))
                })
                || else_children.iter().any(|expr| expr_uses_ident(expr, ident))
        }
        ExprKind::HtmlFor {
            pat,
            iter,
            body_children,
        } => {
            pattern_uses_ident(pat, ident)
                || expr_uses_ident(iter, ident)
                || body_children.iter().any(|expr| expr_uses_ident(expr, ident))
        }
    }
}

fn pattern_uses_ident(pattern: &Pattern, ident: &str) -> bool {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Literal(_) => false,
        PatternKind::Ident(name) => name.name == ident,
        PatternKind::EnumVariant { name, args } => {
            name.name == ident || args.iter().any(|arg| pattern_uses_ident(arg, ident))
        }
        PatternKind::Struct { name, fields } => {
            name.name == ident
                || fields.iter().any(|field| {
                    field.name.name == ident || pattern_uses_ident(&field.pat, ident)
                })
        }
    }
}

fn collect_component(
    decl: &ComponentDecl,
    functions: &mut HashMap<String, FnSigRef>,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) {
    if !register_name(&decl.name.name, decl.name.span, names, diags) {
        return;
    }
    let span = decl.span;
    let mk_ident = |name: &str| Ident {
        name: name.to_string(),
        span,
    };
    let mk_simple = |name: &str| TypeRef {
        kind: TypeRefKind::Simple(mk_ident(name)),
        span,
    };
    let mk_generic = |base: &str, args: Vec<TypeRef>| TypeRef {
        kind: TypeRefKind::Generic {
            base: mk_ident(base),
            args,
        },
        span,
    };
    let mut params = params_ref(&decl.params);
    params.push(ParamRef {
        name: "attrs".to_string(),
        ty: mk_generic("Map", vec![mk_simple("String"), mk_simple("String")]),
        has_default: true,
        span,
    });
    params.push(ParamRef {
        name: "children".to_string(),
        ty: mk_generic("List", vec![mk_simple("Html")]),
        has_default: true,
        span,
    });
    functions.insert(
        decl.name.name.clone(),
        FnSigRef {
            name: decl.name.name.clone(),
            type_params: type_params_ref(&decl.type_params),
            params,
            ret: Some(mk_simple("Html")),
            where_clause: where_constraints_ref(&decl.where_clause),
            span,
        },
    );
}

fn collect_config(
    decl: &ConfigDecl,
    configs: &mut HashMap<String, ConfigInfo>,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) {
    if !register_name(&decl.name.name, decl.name.span, names, diags) {
        return;
    }
    let fields = decl
        .fields
        .iter()
        .map(|field| FieldInfo {
            name: field.name.name.clone(),
            ty: field.ty.clone(),
            span: field.span,
            has_default: true,
        })
        .collect();
    configs.insert(
        decl.name.name.clone(),
        ConfigInfo {
            name: decl.name.name.clone(),
            fields,
            span: decl.span,
        },
    );
}

fn collect_service(
    decl: &ServiceDecl,
    services: &mut HashMap<String, ServiceInfo>,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) {
    if !register_name(&decl.name.name, decl.name.span, names, diags) {
        return;
    }
    let routes = decl
        .routes
        .iter()
        .map(|route| RouteSig {
            span: route.span,
            body_type: route.body_type.clone(),
            ret_type: route.ret_type.clone(),
        })
        .collect();
    services.insert(
        decl.name.name.clone(),
        ServiceInfo {
            name: decl.name.name.clone(),
            routes,
            span: decl.span,
        },
    );
}
