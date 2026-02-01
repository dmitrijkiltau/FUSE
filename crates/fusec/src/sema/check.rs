use std::collections::{HashMap, HashSet};

use crate::ast::{
    Block, Expr, ExprKind, Item, Literal, Pattern, PatternKind, Program, RouteDecl, Stmt, StmtKind,
};
use crate::diag::Diagnostics;
use crate::loader::{ModuleId, ModuleLink, ModuleMap};
use crate::span::Span;

use super::symbols::ModuleSymbols;
use super::types::{FnSig, ParamSig, Ty};

pub struct Checker<'a> {
    module_id: ModuleId,
    symbols: &'a ModuleSymbols,
    modules: &'a ModuleMap,
    module_maps: &'a HashMap<ModuleId, ModuleMap>,
    import_items: &'a HashMap<String, ModuleLink>,
    module_symbols: &'a HashMap<ModuleId, ModuleSymbols>,
    module_import_items: &'a HashMap<ModuleId, HashMap<String, ModuleLink>>,
    diags: &'a mut Diagnostics,
    env: TypeEnv,
    fn_cache: HashMap<(ModuleId, String), FnSig>,
    current_return: Option<Ty>,
}

impl<'a> Checker<'a> {
    pub fn new(
        module_id: ModuleId,
        symbols: &'a ModuleSymbols,
        modules: &'a ModuleMap,
        module_maps: &'a HashMap<ModuleId, ModuleMap>,
        import_items: &'a HashMap<String, ModuleLink>,
        module_symbols: &'a HashMap<ModuleId, ModuleSymbols>,
        module_import_items: &'a HashMap<ModuleId, HashMap<String, ModuleLink>>,
        diags: &'a mut Diagnostics,
    ) -> Self {
        let mut env = TypeEnv::new();
        env.insert_builtin("log");
        env.insert_builtin("db");
        env.insert_builtin("env");
        env.insert_builtin("json");
        env.insert_builtin("time");
        env.insert_builtin("print");
        env.insert_builtin("assert");
        env.insert_builtin("serve");
        env.insert_builtin("errors");
        Self {
            module_id,
            symbols,
            modules,
            module_maps,
            import_items,
            module_symbols,
            module_import_items,
            diags,
            env,
            fn_cache: HashMap::new(),
            current_return: None,
        }
    }

    pub fn check_program(&mut self, program: &Program) {
        for item in &program.items {
            match item {
                Item::Fn(decl) => self.check_fn_decl(decl),
                Item::Config(decl) => self.check_config_decl(decl),
                Item::Service(decl) => self.check_service_decl(decl),
                Item::App(decl) => {
                    self.env.push();
                    let _ = self.check_block(&decl.body);
                    self.env.pop();
                }
                Item::Test(decl) => {
                    self.env.push();
                    let _ = self.check_block(&decl.body);
                    self.env.pop();
                }
                Item::Type(decl) => self.check_type_decl(decl),
                Item::Enum(decl) => self.check_enum_decl(decl),
                Item::Import(_) | Item::Migration(_) => {}
            }
        }
    }

    fn check_type_decl(&mut self, decl: &crate::ast::TypeDecl) {
        for field in &decl.fields {
            let field_ty = self.resolve_type_ref(&field.ty);
            if let Some(default) = &field.default {
                let value_ty = self.check_expr(default);
                if !self.is_assignable(&value_ty, &field_ty) {
                    self.type_mismatch(default.span, &field_ty, &value_ty);
                }
            }
        }
    }

    fn check_enum_decl(&mut self, decl: &crate::ast::EnumDecl) {
        for variant in &decl.variants {
            for ty in &variant.payload {
                let _ = self.resolve_type_ref(ty);
            }
        }
    }

    fn check_config_decl(&mut self, decl: &crate::ast::ConfigDecl) {
        for field in &decl.fields {
            let field_ty = self.resolve_type_ref(&field.ty);
            let value_ty = self.check_expr(&field.value);
            if !self.is_assignable_with_refined(&value_ty, &field_ty) {
                self.type_mismatch(field.value.span, &field_ty, &value_ty);
            }
        }
    }

    fn check_service_decl(&mut self, decl: &crate::ast::ServiceDecl) {
        for route in &decl.routes {
            self.env.push();
            let prev_return = self.current_return.clone();
            let route_ret = self.resolve_type_ref(&route.ret_type);
            self.current_return = Some(route_ret);
            for (name, ty) in self.extract_route_params(route) {
                self.insert_var(&name, ty, false, route.span);
            }
            if let Some(body_ty) = &route.body_type {
                let ty = self.resolve_type_ref(body_ty);
                self.insert_var("body", ty, false, route.span);
            }
            let _ = self.check_block(&route.body);
            self.current_return = prev_return;
            self.env.pop();
        }
    }

    fn check_fn_decl(&mut self, decl: &crate::ast::FnDecl) {
        let sig = self.resolve_fn_sig(decl);
        self.current_return = Some(*sig.ret.clone());
        self.env.push();
        for param in &sig.params {
            self.insert_var(&param.name, param.ty.clone(), false, decl.span);
        }
        let _ = self.check_block(&decl.body);
        self.env.pop();
        self.current_return = None;
    }

    fn resolve_fn_sig(&mut self, decl: &crate::ast::FnDecl) -> FnSig {
        let params = decl
            .params
            .iter()
            .map(|param| ParamSig {
                name: param.name.name.clone(),
                ty: self.resolve_type_ref(&param.ty),
            })
            .collect();
        let ret = decl
            .ret
            .as_ref()
            .map(|ty| self.resolve_type_ref(ty))
            .unwrap_or(Ty::Unit);
        FnSig {
            params,
            ret: Box::new(ret),
        }
    }

    fn fn_sig_in(&mut self, module_id: ModuleId, name: &str) -> Option<FnSig> {
        if let Some(sig) = self.fn_cache.get(&(module_id, name.to_string())) {
            return Some(sig.clone());
        }
        let symbols = self.module_symbols.get(&module_id)?;
        let sig_ref = symbols.functions.get(name)?;
        let params = sig_ref
            .params
            .iter()
            .map(|param| ParamSig {
                name: param.name.clone(),
                ty: self.resolve_type_ref_in(module_id, &param.ty),
            })
            .collect();
        let ret = sig_ref
            .ret
            .as_ref()
            .map(|ty| self.resolve_type_ref_in(module_id, ty))
            .unwrap_or(Ty::Unit);
        let sig = FnSig {
            params,
            ret: Box::new(ret),
        };
        self.fn_cache
            .insert((module_id, name.to_string()), sig.clone());
        Some(sig)
    }

    fn check_block(&mut self, block: &Block) -> Ty {
        self.env.push();
        let mut last = Ty::Unit;
        for stmt in &block.stmts {
            last = self.check_stmt(stmt);
        }
        self.env.pop();
        last
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> Ty {
        match &stmt.kind {
            StmtKind::Let { name, ty, expr } => {
                let value_ty = self.check_expr(expr);
                let final_ty = if let Some(ty_ref) = ty {
                    let ann_ty = self.resolve_type_ref(ty_ref);
                    if !self.is_assignable(&value_ty, &ann_ty) {
                        self.type_mismatch(expr.span, &ann_ty, &value_ty);
                    }
                    ann_ty
                } else {
                    value_ty
                };
                self.insert_var(&name.name, final_ty, false, name.span);
                Ty::Unit
            }
            StmtKind::Var { name, ty, expr } => {
                let value_ty = self.check_expr(expr);
                let final_ty = if let Some(ty_ref) = ty {
                    let ann_ty = self.resolve_type_ref(ty_ref);
                    if !self.is_assignable(&value_ty, &ann_ty) {
                        self.type_mismatch(expr.span, &ann_ty, &value_ty);
                    }
                    ann_ty
                } else {
                    value_ty
                };
                self.insert_var(&name.name, final_ty, true, name.span);
                Ty::Unit
            }
            StmtKind::Assign { target, expr } => {
                let target_ty = self.check_lvalue(target);
                let value_ty = self.check_expr(expr);
                if !self.is_assignable(&value_ty, &target_ty) {
                    self.type_mismatch(expr.span, &target_ty, &value_ty);
                }
                Ty::Unit
            }
            StmtKind::Return { expr } => {
                let value_ty = expr.as_ref().map(|expr| self.check_expr(expr)).unwrap_or(Ty::Unit);
                if let Some(expected) = self.current_return.clone() {
                    if !self.is_assignable(&value_ty, &expected) {
                        self.type_mismatch(stmt.span, &expected, &value_ty);
                    }
                }
                value_ty
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                let cond_ty = self.check_expr(cond);
                self.expect_bool(cond.span, &cond_ty);
                let _ = self.check_block(then_block);
                for (cond, block) in else_if {
                    let cond_ty = self.check_expr(cond);
                    self.expect_bool(cond.span, &cond_ty);
                    let _ = self.check_block(block);
                }
                if let Some(block) = else_block {
                    let _ = self.check_block(block);
                }
                Ty::Unit
            }
            StmtKind::Match { expr, cases } => {
                let expr_ty = self.check_expr(expr);
                for (pat, block) in cases {
                    self.env.push();
                    self.bind_pattern(pat, &expr_ty);
                    let _ = self.check_block(block);
                    self.env.pop();
                }
                Ty::Unit
            }
            StmtKind::For { pat, iter, block } => {
                let iter_ty = self.check_expr(iter);
                let item_ty = match iter_ty {
                    Ty::List(inner) => *inner,
                    Ty::Map(_, value) => *value,
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.diags.error(
                            iter.span,
                            format!("cannot iterate over type {}", other),
                        );
                        Ty::Unknown
                    }
                };
                self.env.push();
                self.bind_pattern(pat, &item_ty);
                let _ = self.check_block(block);
                self.env.pop();
                Ty::Unit
            }
            StmtKind::While { cond, block } => {
                let cond_ty = self.check_expr(cond);
                self.expect_bool(cond.span, &cond_ty);
                let _ = self.check_block(block);
                Ty::Unit
            }
            StmtKind::Expr(expr) => self.check_expr(expr),
            StmtKind::Break | StmtKind::Continue => Ty::Unit,
        }
    }

    fn check_expr(&mut self, expr: &Expr) -> Ty {
        match &expr.kind {
            ExprKind::Literal(lit) => self.ty_from_literal(lit),
            ExprKind::Ident(ident) => self.resolve_ident_expr(ident),
            ExprKind::Unary { op, expr: inner } => {
                let inner_ty = self.check_expr(inner);
                match op {
                    crate::ast::UnaryOp::Neg => {
                        if self.is_numeric(&inner_ty) {
                            inner_ty
                        } else {
                            self.diags.error(expr.span, "unary '-' requires numeric type");
                            Ty::Unknown
                        }
                    }
                    crate::ast::UnaryOp::Not => {
                        if matches!(inner_ty, Ty::Bool | Ty::Unknown) {
                            Ty::Bool
                        } else {
                            self.diags.error(expr.span, "unary '!' requires Bool");
                            Ty::Unknown
                        }
                    }
                }
            }
            ExprKind::Binary { op, left, right } => {
                let left_ty = self.check_expr(left);
                let right_ty = self.check_expr(right);
                self.check_binary(expr.span, op, left_ty, right_ty)
            }
            ExprKind::Call { callee, args } => {
                let callee_ty = self.check_expr(callee);
                match callee_ty {
                    Ty::Fn(sig) => {
                        for arg in args {
                            if arg.name.is_some() {
                                self.diags.error(
                                    arg.span,
                                    "named arguments are not supported for function calls",
                                );
                            }
                        }
                        if args.len() != sig.params.len() {
                            self.diags.error(
                                expr.span,
                                format!(
                                    "expected {} arguments, got {}",
                                    sig.params.len(),
                                    args.len()
                                ),
                            );
                            return *sig.ret;
                        }
                        for (arg, param) in args.iter().zip(sig.params.iter()) {
                            let arg_ty = self.check_expr(&arg.value);
                            if !self.is_assignable(&arg_ty, &param.ty) {
                                self.type_mismatch(arg.span, &param.ty, &arg_ty);
                            }
                        }
                        *sig.ret
                    }
                    Ty::Unknown => Ty::Unknown,
                    _ => {
                        self.diags.error(expr.span, "call target is not a function");
                        Ty::Unknown
                    }
                }
            }
            ExprKind::Member { base, name } => self.check_member(base, name, false),
            ExprKind::OptionalMember { base, name } => self.check_member(base, name, true),
            ExprKind::StructLit { name, fields } => self.check_struct_lit(name, fields, expr.span),
            ExprKind::ListLit(items) => {
                let mut elem_ty = Ty::Unknown;
                for item in items {
                    let item_ty = self.check_expr(item);
                    elem_ty = self.unify_types(item.span, elem_ty, item_ty);
                }
                Ty::List(Box::new(elem_ty))
            }
            ExprKind::MapLit(pairs) => {
                let mut key_ty = Ty::Unknown;
                let mut val_ty = Ty::Unknown;
                for (key, value) in pairs {
                    let k = self.check_expr(key);
                    let v = self.check_expr(value);
                    key_ty = self.unify_types(key.span, key_ty, k);
                    val_ty = self.unify_types(value.span, val_ty, v);
                }
                Ty::Map(Box::new(key_ty), Box::new(val_ty))
            }
            ExprKind::InterpString(parts) => {
                for part in parts {
                    if let crate::ast::InterpPart::Expr(expr) = part {
                        let _ = self.check_expr(expr);
                    }
                }
                Ty::String
            }
            ExprKind::Coalesce { left, right } => {
                let left_ty = self.check_expr(left);
                let right_ty = self.check_expr(right);
                match left_ty {
                    Ty::Option(inner) => {
                        let inner_ty = *inner;
                        self.unify_types(expr.span, inner_ty, right_ty)
                    }
                    _ => self.unify_types(expr.span, left_ty, right_ty),
                }
            }
            ExprKind::BangChain { expr: inner, error } => {
                let inner_ty = self.check_expr(inner);
                let err_ty = error.as_ref().map(|expr| self.check_expr(expr));
                match inner_ty {
                    Ty::Option(inner) => {
                        let err_ty = err_ty.unwrap_or(Ty::Error);
                        self.check_bang_error(expr.span, &err_ty);
                        *inner
                    }
                    Ty::Result(ok, err) => {
                        if let Some(err_ty) = err_ty {
                            self.check_bang_error(expr.span, &err_ty);
                        } else {
                            self.check_bang_error(expr.span, &err);
                        }
                        *ok
                    }
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.diags.error(
                            expr.span,
                            format!("?! expects Option or Result, got {}", other),
                        );
                        Ty::Unknown
                    }
                }
            }
            ExprKind::Spawn { block } => {
                let block_ty = self.check_block(block);
                Ty::Task(Box::new(block_ty))
            }
            ExprKind::Await { expr: inner } => {
                let inner_ty = self.check_expr(inner);
                match inner_ty {
                    Ty::Task(inner) => *inner,
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.diags.error(expr.span, format!("await expects Task, got {}", other));
                        Ty::Unknown
                    }
                }
            }
            ExprKind::Box { expr: inner } => {
                self.check_expr(inner)
            }
        }
    }

    fn check_struct_lit(
        &mut self,
        name: &crate::ast::Ident,
        fields: &[crate::ast::StructField],
        span: Span,
    ) -> Ty {
        if let Some(info) = self.type_info(&name.name) {
            let field_defs = info.fields.clone();
            let mut seen = HashSet::new();
            for field in fields {
                if !seen.insert(field.name.name.clone()) {
                    self.diags.error(field.span, "duplicate field in struct literal");
                    continue;
                }
                let field_info = field_defs.iter().find(|f| f.name == field.name.name);
                if let Some(field_info) = field_info {
                    let value_ty = self.check_expr(&field.value);
                    let field_ty = self.resolve_type_ref(&field_info.ty);
                    if !self.is_assignable_with_refined(&value_ty, &field_ty) {
                        self.type_mismatch(field.span, &field_ty, &value_ty);
                    }
                } else {
                    self.diags.error(
                        field.span,
                        format!("unknown field {} on {}", field.name.name, name.name),
                    );
                }
            }
            for field_info in &field_defs {
                if !seen.contains(&field_info.name) {
                    let field_ty = self.resolve_type_ref(&field_info.ty);
                    if !field_info.has_default && !field_ty.is_optional() {
                        self.diags.error(
                            span,
                            format!("missing field {} for {}", field_info.name, name.name),
                        );
                    }
                }
            }
            Ty::Struct(name.name.clone())
        } else if let Some(info) = self.config_info(&name.name) {
            let field_defs = info.fields.clone();
            let mut seen = HashSet::new();
            for field in fields {
                if !seen.insert(field.name.name.clone()) {
                    self.diags.error(field.span, "duplicate field in config literal");
                    continue;
                }
                let field_info = field_defs.iter().find(|f| f.name == field.name.name);
                if let Some(field_info) = field_info {
                    let value_ty = self.check_expr(&field.value);
                    let field_ty = self.resolve_type_ref(&field_info.ty);
                    if !self.is_assignable_with_refined(&value_ty, &field_ty) {
                        self.type_mismatch(field.span, &field_ty, &value_ty);
                    }
                } else {
                    self.diags.error(
                        field.span,
                        format!("unknown field {} on {}", field.name.name, name.name),
                    );
                }
            }
            Ty::Config(name.name.clone())
        } else {
            self.diags
                .error(name.span, format!("unknown type {}", name.name));
            Ty::Unknown
        }
    }

    fn check_member(&mut self, base: &Expr, name: &crate::ast::Ident, is_optional: bool) -> Ty {
        let base_ty = match &base.kind {
            ExprKind::Ident(ident) => {
                if let Some(var) = self.env.lookup(&ident.name) {
                    var.ty.clone()
                } else if self.modules.contains(&ident.name) {
                    Ty::Module(ident.name.clone())
                } else if self.enum_in_scope(&ident.name) {
                    Ty::Enum(ident.name.clone())
                } else {
                    self.check_expr(base)
                }
            }
            _ => self.check_expr(base),
        };
        let mut inner = base_ty.clone();
        if is_optional {
            match base_ty {
                Ty::Option(inner_ty) => inner = *inner_ty,
                Ty::Unknown => return Ty::Option(Box::new(Ty::Unknown)),
                other => {
                    self.diags.error(
                        base.span,
                        format!("optional access on non-optional {}", other),
                    );
                    return Ty::Unknown;
                }
            }
        }
        let field_ty = match inner {
            Ty::Struct(ref name_ty) => self.lookup_field(name_ty, &name.name, name.span),
            Ty::Config(ref name_ty) => self.lookup_config_field(name_ty, &name.name, name.span),
            Ty::Enum(ref name_ty) => self.lookup_enum_variant(name_ty, name),
            Ty::Module(ref module_name) => self.lookup_module_member(module_name, name),
            Ty::Unknown => Ty::Unknown,
            other => {
                self.diags.error(
                    name.span,
                    format!("type {} has no field {}", other, name.name),
                );
                Ty::Unknown
            }
        };
        if is_optional {
            Ty::Option(Box::new(field_ty))
        } else {
            field_ty
        }
    }

    fn lookup_field(&mut self, type_name: &str, field: &str, span: Span) -> Ty {
        let field_ty = self.type_info(type_name).and_then(|info| {
            info.fields
                .iter()
                .find(|f| f.name == field)
                .map(|field_info| field_info.ty.clone())
        });
        if let Some(field_ty) = field_ty {
            return self.resolve_type_ref(&field_ty);
        }
        self.diags
            .error(span, format!("unknown field {} on {}", field, type_name));
        Ty::Unknown
    }

    fn lookup_config_field(&mut self, type_name: &str, field: &str, span: Span) -> Ty {
        let field_ty = self.config_info(type_name).and_then(|info| {
            info.fields
                .iter()
                .find(|f| f.name == field)
                .map(|field_info| field_info.ty.clone())
        });
        if let Some(field_ty) = field_ty {
            return self.resolve_type_ref(&field_ty);
        }
        self.diags
            .error(span, format!("unknown field {} on {}", field, type_name));
        Ty::Unknown
    }

    fn type_info(&self, name: &str) -> Option<&super::symbols::TypeInfo> {
        if let Some(info) = self.symbols.types.get(name) {
            return Some(info);
        }
        let link = self.import_items.get(name)?;
        let symbols = self.module_symbols.get(&link.id)?;
        symbols.types.get(name)
    }

    fn config_info(&self, name: &str) -> Option<&super::symbols::ConfigInfo> {
        if let Some(info) = self.symbols.configs.get(name) {
            return Some(info);
        }
        let link = self.import_items.get(name)?;
        let symbols = self.module_symbols.get(&link.id)?;
        symbols.configs.get(name)
    }

    fn enum_info(&self, name: &str) -> Option<&super::symbols::EnumInfo> {
        if let Some(info) = self.symbols.enums.get(name) {
            return Some(info);
        }
        let link = self.import_items.get(name)?;
        let symbols = self.module_symbols.get(&link.id)?;
        symbols.enums.get(name)
    }

    fn enum_in_scope(&self, name: &str) -> bool {
        self.enum_info(name).is_some()
    }

    fn resolve_imported_value(&mut self, ident: &crate::ast::Ident, link: &ModuleLink) -> Ty {
        let Some(symbols) = self.module_symbols.get(&link.id) else {
            return Ty::Unknown;
        };
        if symbols.functions.contains_key(&ident.name) {
            if let Some(sig) = self.fn_sig_in(link.id, &ident.name) {
                return Ty::Fn(sig);
            }
            self.diags
                .error(ident.span, format!("unknown function {}", ident.name));
            return Ty::Unknown;
        }
        if symbols.configs.contains_key(&ident.name) {
            return Ty::Config(ident.name.clone());
        }
        if symbols.types.contains_key(&ident.name) || symbols.enums.contains_key(&ident.name) {
            self.diags.error(
                ident.span,
                format!("{} is a type, not a value", ident.name),
            );
            return Ty::Unknown;
        }
        if symbols.services.contains_key(&ident.name) || link.exports.apps.contains(&ident.name) {
            self.diags
                .error(ident.span, format!("{} is not a value", ident.name));
            return Ty::Unknown;
        }
        self.diags
            .error(ident.span, format!("unknown identifier {}", ident.name));
        Ty::Unknown
    }

    fn lookup_enum_variant(&mut self, enum_name: &str, name: &crate::ast::Ident) -> Ty {
        let info = match self.enum_info(enum_name) {
            Some(info) => info,
            None => {
                self.diags
                    .error(name.span, format!("unknown enum {}", enum_name));
                return Ty::Unknown;
            }
        };
        let payload = match info.variants.iter().find(|v| v.name == name.name) {
            Some(variant) => variant.payload.clone(),
            None => {
                self.diags.error(
                    name.span,
                    format!("unknown variant {} for {}", name.name, enum_name),
                );
                return Ty::Unknown;
            }
        };
        if payload.is_empty() {
            return Ty::Enum(enum_name.to_string());
        }
        let params = payload
            .iter()
            .enumerate()
            .map(|(idx, ty)| ParamSig {
                name: format!("arg{idx}"),
                ty: self.resolve_type_ref(ty),
            })
            .collect();
        Ty::Fn(FnSig {
            params,
            ret: Box::new(Ty::Enum(enum_name.to_string())),
        })
    }

    fn lookup_module_member(&mut self, module_name: &str, name: &crate::ast::Ident) -> Ty {
        let Some(link) = self.modules.get(module_name) else {
            self.diags
                .error(name.span, format!("unknown module {}", module_name));
            return Ty::Unknown;
        };
        if !link.exports.contains(&name.name) {
            self.diags.error(
                name.span,
                format!("unknown module member {}.{}", module_name, name.name),
            );
            return Ty::Unknown;
        }
        let Some(symbols) = self.module_symbols.get(&link.id) else {
            self.diags
                .error(name.span, format!("unknown module {}", module_name));
            return Ty::Unknown;
        };
        if symbols.functions.contains_key(&name.name) {
            if let Some(sig) = self.fn_sig_in(link.id, &name.name) {
                return Ty::Fn(sig);
            }
            self.diags.error(
                name.span,
                format!("unknown function {} in {}", name.name, module_name),
            );
            return Ty::Unknown;
        }
        if symbols.configs.contains_key(&name.name) {
            return Ty::Config(name.name.clone());
        }
        if symbols.enums.contains_key(&name.name) {
            return Ty::Enum(name.name.clone());
        }
        if symbols.types.contains_key(&name.name) {
            self.diags.error(
                name.span,
                format!("{}.{} is a type, not a value", module_name, name.name),
            );
            return Ty::Unknown;
        }
        if symbols.services.contains_key(&name.name) || link.exports.apps.contains(&name.name) {
            self.diags.error(
                name.span,
                format!("{}.{} is not a value", module_name, name.name),
            );
            return Ty::Unknown;
        }
        self.diags.error(
            name.span,
            format!("unknown module member {}.{}", module_name, name.name),
        );
        Ty::Unknown
    }

    fn check_lvalue(&mut self, target: &Expr) -> Ty {
        match &target.kind {
            ExprKind::Ident(ident) => {
                if let Some(var) = self.env.lookup(&ident.name) {
                    if !var.mutable {
                        self.diags
                            .error(ident.span, format!("cannot assign to immutable {}", ident.name));
                    }
                    var.ty.clone()
                } else {
                    self.diags
                        .error(ident.span, format!("unknown identifier {}", ident.name));
                    Ty::Unknown
                }
            }
            ExprKind::Member { base, name } => self.check_member(base, name, false),
            ExprKind::OptionalMember { base, name } => self.check_member(base, name, true),
            _ => {
                self.diags.error(target.span, "invalid assignment target");
                Ty::Unknown
            }
        }
    }

    fn resolve_ident_expr(&mut self, ident: &crate::ast::Ident) -> Ty {
        if let Some(var) = self.env.lookup(&ident.name) {
            return var.ty.clone();
        }
        if let Some(link) = self.import_items.get(&ident.name) {
            return self.resolve_imported_value(ident, link);
        }
        if let Some(sig) = self.fn_sig_in(self.module_id, &ident.name) {
            return Ty::Fn(sig);
        }
        if self.symbols.configs.contains_key(&ident.name) {
            return Ty::Config(ident.name.clone());
        }
        if self.symbols.types.contains_key(&ident.name) || self.symbols.enums.contains_key(&ident.name)
        {
            self.diags.error(
                ident.span,
                format!("{} is a type, not a value", ident.name),
            );
            return Ty::Unknown;
        }
        if self.modules.contains(&ident.name) {
            self.diags
                .error(ident.span, format!("{} is a module, not a value", ident.name));
            return Ty::Unknown;
        }
        self.diags
            .error(ident.span, format!("unknown identifier {}", ident.name));
        Ty::Unknown
    }

    fn resolve_type_ref(&mut self, ty: &crate::ast::TypeRef) -> Ty {
        self.resolve_type_ref_in(self.module_id, ty)
    }

    fn resolve_type_ref_in(&mut self, module_id: ModuleId, ty: &crate::ast::TypeRef) -> Ty {
        use crate::ast::TypeRefKind;
        match &ty.kind {
            TypeRefKind::Simple(ident) => {
                self.resolve_simple_type_name_in(module_id, &ident.name, ident.span)
            }
            TypeRefKind::Generic { base, args } => {
                let base_name = base.name.as_str();
                match base_name {
                    "List" => {
                        if args.len() != 1 {
                            self.diags.error(ty.span, "List expects 1 type argument");
                            return Ty::Unknown;
                        }
                        let inner = self.resolve_type_ref_in(module_id, &args[0]);
                        Ty::List(Box::new(inner))
                    }
                    "Map" => {
                        if args.len() != 2 {
                            self.diags.error(ty.span, "Map expects 2 type arguments");
                            return Ty::Unknown;
                        }
                        let key = self.resolve_type_ref_in(module_id, &args[0]);
                        let value = self.resolve_type_ref_in(module_id, &args[1]);
                        Ty::Map(Box::new(key), Box::new(value))
                    }
                    "Option" => {
                        if args.len() != 1 {
                            self.diags.error(ty.span, "Option expects 1 type argument");
                            return Ty::Unknown;
                        }
                        let inner = self.resolve_type_ref_in(module_id, &args[0]);
                        Ty::Option(Box::new(inner))
                    }
                    "Result" => {
                        if args.len() != 2 {
                            self.diags.error(ty.span, "Result expects 2 type arguments");
                            return Ty::Unknown;
                        }
                        let ok = self.resolve_type_ref_in(module_id, &args[0]);
                        let err = self.resolve_type_ref_in(module_id, &args[1]);
                        Ty::Result(Box::new(ok), Box::new(err))
                    }
                    _ => {
                        self.diags
                            .error(base.span, format!("unknown generic type {}", base.name));
                        Ty::Unknown
                    }
                }
            }
            TypeRefKind::Optional(inner) => {
                let inner = self.resolve_type_ref_in(module_id, inner);
                Ty::Option(Box::new(inner))
            }
            TypeRefKind::Result { ok, err } => {
                let ok = self.resolve_type_ref_in(module_id, ok);
                let err = err
                    .as_ref()
                    .map(|err| self.resolve_type_ref_in(module_id, err))
                    .unwrap_or(Ty::Error);
                Ty::Result(Box::new(ok), Box::new(err))
            }
            TypeRefKind::Refined { base, args } => {
                for arg in args {
                    let _ = self.check_expr(arg);
                }
                let base_ty = self.resolve_simple_type_name_in(module_id, &base.name, base.span);
                let repr = format!("{}(...)", base.name);
                Ty::Refined {
                    base: Box::new(base_ty),
                    repr,
                }
            }
        }
    }

    fn resolve_simple_type_name(&mut self, name: &str, span: Span) -> Ty {
        self.resolve_simple_type_name_in(self.module_id, name, span)
    }

    fn resolve_simple_type_name_in(&mut self, module_id: ModuleId, name: &str, span: Span) -> Ty {
        if let Some((module_name, item_name)) = split_qualified_type_name(name) {
            let module_map = self
                .module_maps
                .get(&module_id)
                .unwrap_or(self.modules);
            let Some(link) = module_map.get(module_name) else {
                self.diags
                    .error(span, format!("unknown module {}", module_name));
                return Ty::Unknown;
            };
            let Some(symbols) = self.module_symbols.get(&link.id) else {
                self.diags
                    .error(span, format!("unknown module {}", module_name));
                return Ty::Unknown;
            };
            if symbols.types.contains_key(item_name) {
                return Ty::Struct(item_name.to_string());
            }
            if symbols.enums.contains_key(item_name) {
                return Ty::Enum(item_name.to_string());
            }
            if symbols.configs.contains_key(item_name) {
                return Ty::Config(item_name.to_string());
            }
            if symbols.functions.contains_key(item_name)
                || symbols.services.contains_key(item_name)
                || link.exports.apps.contains(item_name)
            {
                self.diags.error(
                    span,
                    format!("{}.{} is not a type", module_name, item_name),
                );
            } else {
                self.diags.error(
                    span,
                    format!("unknown type {}.{}", module_name, item_name),
                );
            }
            return Ty::Unknown;
        }
        if name.contains('.') {
            self.diags
                .error(span, format!("invalid type path {}", name));
            return Ty::Unknown;
        }
        match name {
            "Int" => Ty::Int,
            "Float" => Ty::Float,
            "Bool" => Ty::Bool,
            "String" => Ty::String,
            "Bytes" => Ty::Bytes,
            "Id" => Ty::Id,
            "Email" => Ty::Email,
            "Error" => Ty::Error,
            _ => {
                let symbols = self
                    .module_symbols
                    .get(&module_id)
                    .unwrap_or(self.symbols);
                if symbols.types.contains_key(name) {
                    return Ty::Struct(name.to_string());
                }
                if symbols.enums.contains_key(name) {
                    return Ty::Enum(name.to_string());
                }
                if symbols.configs.contains_key(name) {
                    return Ty::Config(name.to_string());
                }
                let import_items = self
                    .module_import_items
                    .get(&module_id)
                    .unwrap_or(self.import_items);
                if let Some(link) = import_items.get(name) {
                    if let Some(symbols) = self.module_symbols.get(&link.id) {
                        if symbols.types.contains_key(name) {
                            return Ty::Struct(name.to_string());
                        }
                        if symbols.enums.contains_key(name) {
                            return Ty::Enum(name.to_string());
                        }
                        if symbols.configs.contains_key(name) {
                            return Ty::Config(name.to_string());
                        }
                    }
                }
                self.diags.error(span, format!("unknown type {}", name));
                Ty::Unknown
            }
        }
    }

    fn bind_pattern(&mut self, pat: &Pattern, ty: &Ty) {
        match &pat.kind {
            PatternKind::Wildcard => {}
            PatternKind::Ident(ident) => {
                if self.is_enum_variant_name(ty, &ident.name) {
                    self.check_enum_variant_pattern(ty, &ident.name, &[], pat.span);
                } else {
                    self.insert_var(&ident.name, ty.clone(), false, ident.span);
                }
            }
            PatternKind::Literal(lit) => {
                let lit_ty = self.ty_from_literal(lit);
                if !self.is_assignable(&lit_ty, ty) {
                    self.type_mismatch(pat.span, ty, &lit_ty);
                }
            }
            PatternKind::EnumVariant { name, args } => {
                self.check_enum_variant_pattern(ty, &name.name, args, pat.span);
            }
            PatternKind::Struct { name, fields } => {
                self.check_struct_pattern(ty, name, fields, pat.span);
            }
        }
    }

    fn ty_from_literal(&self, lit: &Literal) -> Ty {
        match lit {
            Literal::Int(_) => Ty::Int,
            Literal::Float(_) => Ty::Float,
            Literal::Bool(_) => Ty::Bool,
            Literal::String(_) => Ty::String,
            Literal::Null => Ty::Option(Box::new(Ty::Unknown)),
        }
    }

    fn check_binary(&mut self, span: Span, op: &crate::ast::BinaryOp, left: Ty, right: Ty) -> Ty {
        use crate::ast::BinaryOp::*;
        match *op {
            Add | Sub | Mul | Div | Mod => {
                if self.is_numeric(&left) && self.is_numeric(&right) {
                    if matches!(left, Ty::Float) || matches!(right, Ty::Float) {
                        Ty::Float
                    } else {
                        Ty::Int
                    }
                } else if matches!(*op, Add)
                    && matches!(left, Ty::String)
                    && matches!(right, Ty::String)
                {
                    Ty::String
                } else if left.is_unknown() || right.is_unknown() {
                    Ty::Unknown
                } else {
                    self.diags.error(span, "binary operator requires numeric types");
                    Ty::Unknown
                }
            }
            Eq | NotEq => {
                if !self.is_assignable(&left, &right) && !self.is_assignable(&right, &left) {
                    self.diags.error(span, "equality comparison on incompatible types");
                }
                Ty::Bool
            }
            Lt | LtEq | Gt | GtEq => {
                if self.is_numeric(&left) && self.is_numeric(&right) {
                    Ty::Bool
                } else if left.is_unknown() || right.is_unknown() {
                    Ty::Bool
                } else {
                    self.diags.error(span, "comparison requires numeric types");
                    Ty::Unknown
                }
            }
            And | Or => {
                if matches!(left, Ty::Bool) && matches!(right, Ty::Bool) {
                    Ty::Bool
                } else if left.is_unknown() || right.is_unknown() {
                    Ty::Bool
                } else {
                    self.diags.error(span, "logical operators require Bool");
                    Ty::Unknown
                }
            }
            Range => {
                if self.is_numeric(&left) && self.is_numeric(&right) {
                    Ty::Range(Box::new(left))
                } else if left.is_unknown() || right.is_unknown() {
                    Ty::Range(Box::new(Ty::Unknown))
                } else {
                    self.diags.error(span, "range requires numeric types");
                    Ty::Unknown
                }
            }
        }
    }

    fn unify_types(&mut self, span: Span, left: Ty, right: Ty) -> Ty {
        if left.is_unknown() {
            return right;
        }
        if right.is_unknown() {
            return left;
        }
        if self.is_assignable(&right, &left) {
            return left;
        }
        if self.is_assignable(&left, &right) {
            return right;
        }
        self.diags.error(span, "incompatible types");
        Ty::Unknown
    }

    fn is_numeric(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Int | Ty::Float => true,
            Ty::Refined { base, .. } => self.is_numeric(base),
            _ => false,
        }
    }

    fn is_assignable(&self, value: &Ty, target: &Ty) -> bool {
        if target == value {
            return true;
        }
        match (value, target) {
            (Ty::Refined { base, .. }, _) => self.is_assignable(base, target),
            (Ty::Result(value_ok, value_err), Ty::Result(target_ok, target_err)) => {
                self.is_assignable(value_ok, target_ok) && self.is_assignable(value_err, target_err)
            }
            (_, Ty::Result(target_ok, _)) => self.is_assignable(value, target_ok),
            (Ty::Result(_, _), _) => false,
            (Ty::Option(value_inner), Ty::Option(target_inner)) => {
                self.is_assignable(value_inner, target_inner)
            }
            (Ty::Option(_), _) => false,
            (_, Ty::Option(inner)) => self.is_assignable(value, inner),
            (Ty::Unknown, _) | (_, Ty::Unknown) => true,
            _ => false,
        }
    }

    fn is_assignable_with_refined(&self, value: &Ty, target: &Ty) -> bool {
        if self.is_assignable(value, target) {
            return true;
        }
        match target {
            Ty::Refined { base, .. } => self.is_assignable(value, base),
            _ => false,
        }
    }

    fn type_mismatch(&mut self, span: Span, expected: &Ty, found: &Ty) {
        self.diags.error(
            span,
            format!("type mismatch: expected {}, found {}", expected, found),
        );
    }

    fn check_bang_error(&mut self, span: Span, err_ty: &Ty) {
        let expected_err = match &self.current_return {
            Some(Ty::Result(_, err)) => Some(*err.clone()),
            Some(Ty::Unknown) => None,
            Some(other) => {
                self.diags.error(
                    span,
                    format!("?! used in non-fallible function returning {}", other),
                );
                return;
            }
            None => {
                self.diags.error(span, "?! used outside of a function");
                return;
            }
        };
        if let Some(expected_err) = expected_err {
            if !self.is_assignable(err_ty, &expected_err) {
                self.type_mismatch(span, &expected_err, err_ty);
            }
        }
    }

    fn is_enum_variant_name(&self, ty: &Ty, name: &str) -> bool {
        match ty {
            Ty::Enum(enum_name) => self
                .enum_info(enum_name)
                .map(|info| info.variants.iter().any(|v| v.name == name))
                .unwrap_or(false),
            Ty::Option(_) => matches!(name, "Some" | "None"),
            Ty::Result(_, _) => matches!(name, "Ok" | "Err"),
            _ => false,
        }
    }

    fn check_enum_variant_pattern(
        &mut self,
        ty: &Ty,
        name: &str,
        args: &[Pattern],
        span: Span,
    ) {
        match ty {
            Ty::Enum(enum_name) => {
                let info = match self.enum_info(enum_name) {
                    Some(info) => info,
                    None => {
                        self.diags
                            .error(span, format!("unknown enum {}", enum_name));
                        return;
                    }
                };
                let payload = match info.variants.iter().find(|v| v.name == name) {
                    Some(variant) => variant.payload.clone(),
                    None => {
                        self.diags.error(
                            span,
                            format!("unknown variant {} for {}", name, enum_name),
                        );
                        return;
                    }
                };
                if payload.len() != args.len() {
                    self.diags.error(
                        span,
                        format!(
                            "expected {} pattern args for {}, got {}",
                            payload.len(),
                            name,
                            args.len()
                        ),
                    );
                }
                for (pat, ty_ref) in args.iter().zip(payload.iter()) {
                    let payload_ty = self.resolve_type_ref(ty_ref);
                    self.bind_pattern(pat, &payload_ty);
                }
            }
            Ty::Option(inner) => match name {
                "Some" => {
                    if args.len() != 1 {
                        self.diags.error(span, "Some expects 1 pattern");
                        return;
                    }
                    let inner = *inner.clone();
                    self.bind_pattern(&args[0], &inner);
                }
                "None" => {
                    if !args.is_empty() {
                        self.diags.error(span, "None expects no patterns");
                    }
                }
                _ => {
                    self.diags.error(span, format!("unknown variant {}", name));
                }
            },
            Ty::Result(ok, err) => match name {
                "Ok" => {
                    if args.len() != 1 {
                        self.diags.error(span, "Ok expects 1 pattern");
                        return;
                    }
                    let ok = *ok.clone();
                    self.bind_pattern(&args[0], &ok);
                }
                "Err" => {
                    if args.len() != 1 {
                        self.diags.error(span, "Err expects 1 pattern");
                        return;
                    }
                    let err = *err.clone();
                    self.bind_pattern(&args[0], &err);
                }
                _ => {
                    self.diags.error(span, format!("unknown variant {}", name));
                }
            },
            Ty::Unknown => {
                for pat in args {
                    self.bind_pattern(pat, &Ty::Unknown);
                }
            }
            other => {
                self.diags.error(
                    span,
                    format!("{} is not matchable as enum variant {}", other, name),
                );
            }
        }
    }

    fn check_struct_pattern(
        &mut self,
        ty: &Ty,
        name: &crate::ast::Ident,
        fields: &[crate::ast::PatternField],
        span: Span,
    ) {
        let target = match ty {
            Ty::Struct(target) | Ty::Config(target) => {
                if target != &name.name {
                    self.diags.error(
                        span,
                        format!("pattern {} does not match {}", name.name, target),
                    );
                }
                target.clone()
            }
            Ty::Unknown => name.name.clone(),
            other => {
                self.diags.error(
                    span,
                    format!("{} is not matchable as struct {}", other, name.name),
                );
                name.name.clone()
            }
        };
        let field_defs = if let Some(info) = self.type_info(&target) {
            info.fields.clone()
        } else if let Some(info) = self.config_info(&target) {
            info.fields.clone()
        } else {
            self.diags
                .error(span, format!("unknown struct {}", name.name));
            return;
        };
        let mut seen = HashSet::new();
        for field in fields {
            if !seen.insert(field.name.name.clone()) {
                self.diags.error(field.span, "duplicate pattern field");
                continue;
            }
            let field_info = field_defs.iter().find(|f| f.name == field.name.name);
            if let Some(field_info) = field_info {
                let field_ty = self.resolve_type_ref(&field_info.ty);
                self.bind_pattern(&field.pat, &field_ty);
            } else {
                self.diags.error(
                    field.span,
                    format!("unknown field {} on {}", field.name.name, name.name),
                );
            }
        }
    }

    fn expect_bool(&mut self, span: Span, ty: &Ty) {
        if matches!(ty, Ty::Bool | Ty::Unknown) {
            return;
        }
        self.diags.error(span, "expected Bool condition");
    }

    fn insert_var(&mut self, name: &str, ty: Ty, mutable: bool, span: Span) {
        if let Err(msg) = self.env.insert(name, ty, mutable) {
            self.diags.error(span, msg);
        }
    }

    fn extract_route_params(&mut self, route: &RouteDecl) -> Vec<(String, Ty)> {
        let mut out = Vec::new();
        let path = &route.path.value;
        let mut idx = 0usize;
        while let Some(start) = path[idx..].find('{') {
            let start_idx = idx + start;
            if let Some(end) = path[start_idx + 1..].find('}') {
                let end_idx = start_idx + 1 + end;
                let inner = &path[start_idx + 1..end_idx];
                let mut parts = inner.splitn(2, ':');
                let name = parts.next().unwrap_or("").trim();
                let ty_name = parts.next().unwrap_or("").trim();
                if name.is_empty() || ty_name.is_empty() {
                    self.diags
                        .error(route.path.span, "invalid route parameter");
                } else if !is_simple_ident(ty_name) {
                    self.diags.error(
                        route.path.span,
                        format!("unsupported route param type {}", ty_name),
                    );
                } else {
                    let ty = self.resolve_simple_type_name(ty_name, route.path.span);
                    out.push((name.to_string(), ty));
                }
                idx = end_idx + 1;
            } else {
                self.diags.error(route.path.span, "unclosed route parameter");
                break;
            }
        }
        out
    }
}

fn split_qualified_type_name(name: &str) -> Option<(&str, &str)> {
    let mut parts = name.split('.');
    let module = parts.next()?;
    let item = parts.next()?;
    if module.is_empty() || item.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((module, item))
}

fn is_simple_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[derive(Default)]
struct TypeEnv {
    scopes: Vec<Scope>,
}

impl TypeEnv {
    fn new() -> Self {
        Self {
            scopes: vec![Scope::default()],
        }
    }

    fn push(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn pop(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn insert_builtin(&mut self, name: &str) {
        let _ = self.insert(name, Ty::Unknown, false);
    }

    fn insert(&mut self, name: &str, ty: Ty, mutable: bool) -> Result<(), String> {
        if let Some(scope) = self.scopes.last_mut() {
            if scope.vars.contains_key(name) {
                return Err(format!("duplicate binding: {name}"));
            }
            scope.vars.insert(
                name.to_string(),
                VarInfo {
                    ty,
                    mutable,
                },
            );
        }
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<&VarInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(var) = scope.vars.get(name) {
                return Some(var);
            }
        }
        None
    }
}

#[derive(Default)]
struct Scope {
    vars: HashMap<String, VarInfo>,
}

struct VarInfo {
    ty: Ty,
    mutable: bool,
}
