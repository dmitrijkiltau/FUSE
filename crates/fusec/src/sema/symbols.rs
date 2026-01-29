use std::collections::HashMap;

use crate::ast::{ConfigDecl, EnumDecl, FnDecl, ImportDecl, ImportSpec, Item, Program, ServiceDecl, TypeDecl, TypeRef};
use crate::diag::Diagnostics;
use crate::span::Span;

#[derive(Clone, Debug)]
pub struct ModuleSymbols {
    pub types: HashMap<String, TypeInfo>,
    pub enums: HashMap<String, EnumInfo>,
    pub functions: HashMap<String, FnSigRef>,
    pub configs: HashMap<String, ConfigInfo>,
    pub services: HashMap<String, ServiceInfo>,
    pub imports: HashMap<String, ImportInfo>,
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
pub struct FnSigRef {
    pub name: String,
    pub params: Vec<ParamRef>,
    pub ret: Option<TypeRef>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ParamRef {
    pub name: String,
    pub ty: TypeRef,
    pub span: Span,
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
}

pub fn collect(program: &Program, diags: &mut Diagnostics) -> ModuleSymbols {
    let mut types = HashMap::new();
    let mut enums = HashMap::new();
    let mut functions = HashMap::new();
    let mut configs = HashMap::new();
    let mut services = HashMap::new();
    let mut imports = HashMap::new();
    let mut names: HashMap<String, Span> = HashMap::new();

    for item in &program.items {
        match item {
            Item::Import(decl) => collect_import(decl, &mut imports, &mut names, diags),
            Item::Type(decl) => collect_type(decl, &mut types, &mut names, diags),
            Item::Enum(decl) => collect_enum(decl, &mut enums, &mut names, diags),
            Item::Fn(decl) => collect_fn(decl, &mut functions, &mut names, diags),
            Item::Config(decl) => collect_config(decl, &mut configs, &mut names, diags),
            Item::Service(decl) => collect_service(decl, &mut services, &mut names, diags),
            _ => {}
        }
    }

    ModuleSymbols {
        types,
        enums,
        functions,
        configs,
        services,
        imports,
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
            if register_name(&name.name, name.span, names, diags) {
                imports.insert(
                    name.name.clone(),
                    ImportInfo {
                        name: name.name.clone(),
                        kind: ImportKind::Module,
                        path: Some(path.value.clone()),
                        span: name.span,
                    },
                );
            }
        }
        ImportSpec::NamedFrom { names: import_names, path } => {
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
        ImportSpec::AliasFrom { name: _name, alias, path } => {
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

fn collect_fn(
    decl: &FnDecl,
    functions: &mut HashMap<String, FnSigRef>,
    names: &mut HashMap<String, Span>,
    diags: &mut Diagnostics,
) {
    if !register_name(&decl.name.name, decl.name.span, names, diags) {
        return;
    }
    let params = decl
        .params
        .iter()
        .map(|param| ParamRef {
            name: param.name.name.clone(),
            ty: param.ty.clone(),
            span: param.span,
        })
        .collect();
    functions.insert(
        decl.name.name.clone(),
        FnSigRef {
            name: decl.name.name.clone(),
            params,
            ret: decl.ret.clone(),
            span: decl.span,
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
