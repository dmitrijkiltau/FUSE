use std::collections::{HashMap, HashSet};

use crate::ast::{
    Block, CallArg, Capability, Expr, ExprKind, Item, Literal, Pattern, PatternKind, Program,
    RouteDecl, Stmt, StmtKind,
};
use crate::diag::Diagnostics;
use crate::frontend::html_shorthand::{CanonicalizationPhase, validate_named_args_for_phase};
use crate::frontend::html_tag_builtin::should_use_html_tag_builtin;
use crate::html_tags::{self, HtmlTagKind};
use crate::loader::{ModuleId, ModuleLink, ModuleMap};
use crate::refinement::{
    NumberLiteral, RefinementConstraint, base_is_string_like, parse_constraints,
};
use crate::span::Span;

use super::symbols::ModuleSymbols;
use super::types::{FnSig, ParamSig, Ty};

pub struct Checker<'a> {
    module_id: ModuleId,
    symbols: &'a ModuleSymbols,
    modules: &'a ModuleMap,
    module_maps: &'a HashMap<ModuleId, ModuleMap>,
    import_items: &'a HashMap<String, ModuleLink>,
    module_capabilities: &'a HashMap<ModuleId, HashSet<Capability>>,
    module_symbols: &'a HashMap<ModuleId, ModuleSymbols>,
    module_import_items: &'a HashMap<ModuleId, HashMap<String, ModuleLink>>,
    diags: &'a mut Diagnostics,
    env: TypeEnv,
    fn_cache: HashMap<(ModuleId, String), FnSig>,
    current_return: Option<Ty>,
    spawn_scope_markers: Vec<usize>,
    declared_capabilities: HashSet<Capability>,
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
        module_capabilities: &'a HashMap<ModuleId, HashSet<Capability>>,
        diags: &'a mut Diagnostics,
    ) -> Self {
        let mut env = TypeEnv::new();
        env.insert_builtin("log");
        env.insert_builtin_with_ty("db", Ty::External("db".to_string()));
        env.insert_builtin("env");
        env.insert_builtin("json");
        env.insert_builtin("time");
        env.insert_builtin("print");
        env.insert_builtin_with_ty(
            "input",
            Ty::Fn(FnSig {
                params: vec![ParamSig {
                    name: "prompt".to_string(),
                    ty: Ty::String,
                    has_default: true,
                }],
                ret: Box::new(Ty::String),
            }),
        );
        env.insert_builtin("assert");
        env.insert_builtin_with_ty(
            "asset",
            Ty::Fn(FnSig {
                params: vec![ParamSig {
                    name: "path".to_string(),
                    ty: Ty::String,
                    has_default: false,
                }],
                ret: Box::new(Ty::String),
            }),
        );
        env.insert_builtin("serve");
        env.insert_builtin("crypto");
        env.insert_builtin_with_ty("task", Ty::External("task".to_string()));
        env.insert_builtin_with_ty("html", Ty::External("html".to_string()));
        env.insert_builtin_with_ty("svg", Ty::External("svg".to_string()));
        env.insert_builtin("errors");
        let declared_capabilities = module_capabilities
            .get(&module_id)
            .cloned()
            .unwrap_or_default();
        Self {
            module_id,
            symbols,
            modules,
            module_maps,
            import_items,
            module_capabilities,
            module_symbols,
            module_import_items,
            diags,
            env,
            fn_cache: HashMap::new(),
            current_return: None,
            spawn_scope_markers: Vec::new(),
            declared_capabilities,
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
                has_default: param.default.is_some(),
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
                has_default: param.has_default,
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

    fn require_capability(&mut self, span: Span, capability: Capability, detail: &str) {
        if self.declared_capabilities.contains(&capability) {
            return;
        }
        let name = capability.as_str();
        self.diags.error(
            span,
            format!(
                "{detail} requires capability {name}; add `requires {name}` at module top-level"
            ),
        );
    }

    fn require_module_capabilities_for_call(
        &mut self,
        span: Span,
        callee_module_id: ModuleId,
        callee_label: &str,
    ) {
        let Some(required_caps) = self.module_capabilities.get(&callee_module_id) else {
            return;
        };
        let mut caps: Vec<_> = required_caps.iter().copied().collect();
        caps.sort_by_key(|cap| cap.as_str());
        for capability in caps {
            if self.declared_capabilities.contains(&capability) {
                continue;
            }
            let name = capability.as_str();
            self.diags.error(
                span,
                format!(
                    "call to {callee_label} leaks capability {name}; add `requires {name}` at module top-level"
                ),
            );
        }
    }

    fn enforce_call_capabilities(&mut self, callee: &Expr, span: Span) {
        match &callee.kind {
            ExprKind::Ident(ident) => {
                if let Some(capability) = capability_for_ident_call(&ident.name) {
                    self.require_capability(span, capability, &format!("call {}", ident.name));
                }
                let imported_fn_module = self.import_items.get(&ident.name).and_then(|link| {
                    if link.exports.functions.contains(&ident.name) {
                        Some(link.id)
                    } else {
                        None
                    }
                });
                if let Some(module_id) = imported_fn_module {
                    self.require_module_capabilities_for_call(span, module_id, &ident.name);
                }
            }
            ExprKind::Member { base, name } => {
                let ExprKind::Ident(base_ident) = &base.kind else {
                    return;
                };
                match base_ident.name.as_str() {
                    "db" if matches!(name.name.as_str(), "exec" | "query" | "one" | "from") => {
                        self.require_capability(span, Capability::Db, "db call")
                    }
                    "time" => self.require_capability(span, Capability::Time, "time call"),
                    "crypto" => self.require_capability(span, Capability::Crypto, "crypto call"),
                    _ => {}
                }
                let imported_module_fn = self.modules.get(&base_ident.name).and_then(|link| {
                    if link.exports.functions.contains(&name.name) {
                        Some(link.id)
                    } else {
                        None
                    }
                });
                if let Some(module_id) = imported_module_fn {
                    self.require_module_capabilities_for_call(
                        span,
                        module_id,
                        &format!("{}.{}", base_ident.name, name.name),
                    );
                }
            }
            _ => {}
        }
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

    fn in_spawn_scope(&self) -> bool {
        !self.spawn_scope_markers.is_empty()
    }

    fn is_outer_capture(&self, name: &str) -> bool {
        let Some(marker) = self.spawn_scope_markers.last().copied() else {
            return false;
        };
        let Some((_, depth)) = self.env.lookup_with_depth(name) else {
            return false;
        };
        depth < marker
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
                if self.in_spawn_scope() {
                    if let Some(root) = lvalue_root_name(target) {
                        if self.is_outer_capture(root) {
                            self.diags.error(
                                target.span,
                                format!("spawn blocks cannot mutate captured outer state ({root})"),
                            );
                        }
                    }
                }
                let target_ty = self.check_lvalue(target);
                let value_ty = self.check_expr(expr);
                if !self.is_assignable(&value_ty, &target_ty) {
                    self.type_mismatch(expr.span, &target_ty, &value_ty);
                }
                Ty::Unit
            }
            StmtKind::Return { expr } => {
                let value_ty = expr
                    .as_ref()
                    .map(|expr| self.check_expr(expr))
                    .unwrap_or(Ty::Unit);
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
                        self.diags
                            .error(iter.span, format!("cannot iterate over type {}", other));
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
                            self.diags
                                .error(expr.span, "unary '-' requires numeric type");
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
                let uses_html_block = args.iter().any(|arg| arg.is_block_sugar);
                if let ExprKind::Ident(ident) = &callee.kind {
                    if self.should_use_html_tag_builtin(&ident.name)
                        || force_html_input_tag_call(&ident.name, args)
                    {
                        return self.check_html_tag_call(expr.span, &ident.name, args);
                    }
                }
                if self.in_spawn_scope() {
                    if let Some(forbidden) = spawn_forbidden_builtin(callee) {
                        self.diags.error(
                            expr.span,
                            format!("spawn blocks cannot call side-effect builtin {forbidden}"),
                        );
                    }
                }
                self.enforce_call_capabilities(callee, expr.span);
                if let ExprKind::Member { base, name } = &callee.kind {
                    if let ExprKind::Ident(ident) = &base.kind {
                        if ident.name == "db"
                            && matches!(name.name.as_str(), "exec" | "query" | "one")
                        {
                            if uses_html_block {
                                self.diags.error(
                                    expr.span,
                                    "html block form requires a function that returns Html",
                                );
                            }
                            if args.len() < 1 || args.len() > 2 {
                                self.diags.error(expr.span, "db.* expects 1 or 2 arguments");
                            }
                            if let Some(first) = args.get(0) {
                                let arg_ty = self.check_expr(&first.value);
                                if !self.is_assignable(&arg_ty, &Ty::String) {
                                    self.type_mismatch(first.span, &Ty::String, &arg_ty);
                                }
                            }
                            if let Some(second) = args.get(1) {
                                let arg_ty = self.check_expr(&second.value);
                                match arg_ty {
                                    Ty::List(_) | Ty::Unknown => {}
                                    other => {
                                        self.diags.error(
                                            second.span,
                                            format!("expected List for params, got {other}"),
                                        );
                                    }
                                }
                            }
                            return match name.name.as_str() {
                                "exec" => Ty::Unit,
                                "query" => Ty::List(Box::new(Ty::Map(
                                    Box::new(Ty::String),
                                    Box::new(Ty::Unknown),
                                ))),
                                "one" => Ty::Option(Box::new(Ty::Map(
                                    Box::new(Ty::String),
                                    Box::new(Ty::Unknown),
                                ))),
                                _ => Ty::Unknown,
                            };
                        }
                    }
                }
                let callee_ty = self.check_expr(callee);
                match callee_ty {
                    Ty::Fn(sig) => {
                        if uses_html_block && !matches!(sig.ret.as_ref(), Ty::Html | Ty::Unknown) {
                            self.diags.error(
                                expr.span,
                                "html block form requires a function that returns Html",
                            );
                        }
                        for arg in args {
                            if arg.name.is_some() {
                                self.diags.error(
                                    arg.span,
                                    "named arguments are not supported for function calls",
                                );
                            }
                        }
                        let provided = args.len();
                        let total = sig.params.len();
                        let missing_are_defaulted = provided <= total
                            && sig.params[provided..].iter().all(|param| param.has_default);
                        if !missing_are_defaulted {
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
            ExprKind::Index { base, index } => self.check_index(base, index, false),
            ExprKind::OptionalIndex { base, index } => self.check_index(base, index, true),
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
                let marker = self.env.depth();
                self.spawn_scope_markers.push(marker);
                let block_ty = self.check_block(block);
                self.spawn_scope_markers.pop();
                Ty::Task(Box::new(block_ty))
            }
            ExprKind::Await { expr: inner } => {
                let inner_ty = self.check_expr(inner);
                match inner_ty {
                    Ty::Task(inner) => *inner,
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.diags
                            .error(expr.span, format!("await expects Task, got {}", other));
                        Ty::Unknown
                    }
                }
            }
            ExprKind::Box { expr: inner } => {
                if self.in_spawn_scope() {
                    self.diags.error(
                        expr.span,
                        "spawn blocks cannot use box values (capture/share is forbidden)",
                    );
                }
                let inner_ty = self.check_expr(inner);
                Ty::Boxed(Box::new(inner_ty))
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
                    self.diags
                        .error(field.span, "duplicate field in struct literal");
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
                    self.diags
                        .error(field.span, "duplicate field in config literal");
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
        let mut inner = Self::unbox_transparent(base_ty.clone());
        if is_optional {
            match inner {
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
            Ty::External(ref external) => self.lookup_external_member(external, name),
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

    fn check_index(&mut self, base: &Expr, index: &Expr, is_optional: bool) -> Ty {
        let base_ty = self.check_expr(base);
        let mut inner = Self::unbox_transparent(base_ty.clone());
        if is_optional {
            match inner {
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
        let index_ty = self.check_expr(index);
        let value_ty = match inner {
            Ty::List(elem_ty) => {
                if !matches!(index_ty, Ty::Unknown) && !self.is_int_like(&index_ty) {
                    self.diags.error(index.span, "list index must be Int");
                }
                *elem_ty
            }
            Ty::Map(key_ty, value_ty) => {
                if !self.is_assignable(&index_ty, &key_ty) {
                    self.diags.error(index.span, "map index type mismatch");
                }
                *value_ty
            }
            Ty::Unknown => Ty::Unknown,
            other => {
                self.diags
                    .error(base.span, format!("type {} is not indexable", other));
                Ty::Unknown
            }
        };
        if is_optional {
            Ty::Option(Box::new(value_ty))
        } else {
            value_ty
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

    fn lookup_external_member(&mut self, external: &str, name: &crate::ast::Ident) -> Ty {
        match external {
            "db" => self.lookup_db_member(name),
            "query" => self.lookup_query_member(name),
            "task" => self.lookup_task_member(name),
            "html" => self.lookup_html_member(name),
            "svg" => self.lookup_svg_member(name),
            _ => {
                self.diags.error(
                    name.span,
                    format!("{} has no field {}", external, name.name),
                );
                Ty::Unknown
            }
        }
    }

    fn lookup_html_member(&mut self, name: &crate::ast::Ident) -> Ty {
        match name.name.as_str() {
            "text" | "raw" => Ty::Fn(FnSig {
                params: vec![ParamSig {
                    name: "value".to_string(),
                    ty: Ty::String,
                    has_default: false,
                }],
                ret: Box::new(Ty::Html),
            }),
            "node" => Ty::Fn(FnSig {
                params: vec![
                    ParamSig {
                        name: "name".to_string(),
                        ty: Ty::String,
                        has_default: false,
                    },
                    ParamSig {
                        name: "attrs".to_string(),
                        ty: Ty::Map(Box::new(Ty::String), Box::new(Ty::String)),
                        has_default: false,
                    },
                    ParamSig {
                        name: "children".to_string(),
                        ty: Ty::List(Box::new(Ty::Html)),
                        has_default: false,
                    },
                ],
                ret: Box::new(Ty::Html),
            }),
            "render" => Ty::Fn(FnSig {
                params: vec![ParamSig {
                    name: "value".to_string(),
                    ty: Ty::Html,
                    has_default: false,
                }],
                ret: Box::new(Ty::String),
            }),
            _ => {
                self.diags
                    .error(name.span, format!("unknown html method {}", name.name));
                Ty::Unknown
            }
        }
    }

    fn lookup_svg_member(&mut self, name: &crate::ast::Ident) -> Ty {
        match name.name.as_str() {
            "inline" => Ty::Fn(FnSig {
                params: vec![ParamSig {
                    name: "name".to_string(),
                    ty: Ty::String,
                    has_default: false,
                }],
                ret: Box::new(Ty::Html),
            }),
            _ => {
                self.diags
                    .error(name.span, format!("unknown svg method {}", name.name));
                Ty::Unknown
            }
        }
    }

    fn lookup_db_member(&mut self, name: &crate::ast::Ident) -> Ty {
        let sql_arg = ParamSig {
            name: "sql".to_string(),
            ty: Ty::String,
            has_default: false,
        };
        let row_ty = Ty::Map(Box::new(Ty::String), Box::new(Ty::Unknown));
        match name.name.as_str() {
            "exec" => Ty::Fn(FnSig {
                params: vec![sql_arg.clone()],
                ret: Box::new(Ty::Unit),
            }),
            "query" => Ty::Fn(FnSig {
                params: vec![sql_arg.clone()],
                ret: Box::new(Ty::List(Box::new(row_ty))),
            }),
            "one" => Ty::Fn(FnSig {
                params: vec![sql_arg],
                ret: Box::new(Ty::Option(Box::new(row_ty))),
            }),
            "from" => Ty::Fn(FnSig {
                params: vec![ParamSig {
                    name: "table".to_string(),
                    ty: Ty::String,
                    has_default: false,
                }],
                ret: Box::new(Ty::External("query".to_string())),
            }),
            _ => {
                self.diags
                    .error(name.span, format!("unknown db method {}", name.name));
                Ty::Unknown
            }
        }
    }

    fn lookup_query_member(&mut self, name: &crate::ast::Ident) -> Ty {
        let row_ty = Ty::Map(Box::new(Ty::String), Box::new(Ty::Unknown));
        match name.name.as_str() {
            "select" => Ty::Fn(FnSig {
                params: vec![ParamSig {
                    name: "columns".to_string(),
                    ty: Ty::List(Box::new(Ty::String)),
                    has_default: false,
                }],
                ret: Box::new(Ty::External("query".to_string())),
            }),
            "where" => Ty::Fn(FnSig {
                params: vec![
                    ParamSig {
                        name: "column".to_string(),
                        ty: Ty::String,
                        has_default: false,
                    },
                    ParamSig {
                        name: "op".to_string(),
                        ty: Ty::String,
                        has_default: false,
                    },
                    ParamSig {
                        name: "value".to_string(),
                        ty: Ty::Unknown,
                        has_default: false,
                    },
                ],
                ret: Box::new(Ty::External("query".to_string())),
            }),
            "order_by" => Ty::Fn(FnSig {
                params: vec![
                    ParamSig {
                        name: "column".to_string(),
                        ty: Ty::String,
                        has_default: false,
                    },
                    ParamSig {
                        name: "dir".to_string(),
                        ty: Ty::String,
                        has_default: false,
                    },
                ],
                ret: Box::new(Ty::External("query".to_string())),
            }),
            "limit" => Ty::Fn(FnSig {
                params: vec![ParamSig {
                    name: "n".to_string(),
                    ty: Ty::Int,
                    has_default: false,
                }],
                ret: Box::new(Ty::External("query".to_string())),
            }),
            "one" => Ty::Fn(FnSig {
                params: vec![],
                ret: Box::new(Ty::Option(Box::new(row_ty.clone()))),
            }),
            "all" => Ty::Fn(FnSig {
                params: vec![],
                ret: Box::new(Ty::List(Box::new(row_ty.clone()))),
            }),
            "exec" => Ty::Fn(FnSig {
                params: vec![],
                ret: Box::new(Ty::Unit),
            }),
            "sql" => Ty::Fn(FnSig {
                params: vec![],
                ret: Box::new(Ty::String),
            }),
            "params" => Ty::Fn(FnSig {
                params: vec![],
                ret: Box::new(Ty::List(Box::new(Ty::Unknown))),
            }),
            _ => {
                self.diags
                    .error(name.span, format!("unknown query method {}", name.name));
                Ty::Unknown
            }
        }
    }

    fn lookup_task_member(&mut self, name: &crate::ast::Ident) -> Ty {
        self.diags.error(
            name.span,
            format!(
                "task.{} was removed in v0.2.0; use spawn + await instead",
                name.name
            ),
        );
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
            self.diags
                .error(ident.span, format!("{} is a type, not a value", ident.name));
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
                has_default: false,
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
                    if let Ty::Boxed(inner) = &var.ty {
                        return *inner.clone();
                    }
                    if !var.mutable {
                        self.diags.error(
                            ident.span,
                            format!("cannot assign to immutable {}", ident.name),
                        );
                    }
                    var.ty.clone()
                } else {
                    self.diags
                        .error(ident.span, format!("unknown identifier {}", ident.name));
                    Ty::Unknown
                }
            }
            ExprKind::Member { base, name } => {
                let base_ty = Self::unbox_transparent(self.check_expr(base));
                match base_ty {
                    Ty::Struct(name_ty) => self.lookup_field(&name_ty, &name.name, name.span),
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.diags.error(
                            target.span,
                            format!("assignment target must be a struct field (got {other})"),
                        );
                        Ty::Unknown
                    }
                }
            }
            ExprKind::OptionalMember { base, name } => {
                let base_ty = Self::unbox_transparent(self.check_expr(base));
                let inner = match base_ty {
                    Ty::Option(inner) => *inner,
                    Ty::Unknown => return Ty::Unknown,
                    other => {
                        self.diags.error(
                            base.span,
                            format!("optional access on non-optional {}", other),
                        );
                        return Ty::Unknown;
                    }
                };
                match inner {
                    Ty::Struct(name_ty) => self.lookup_field(&name_ty, &name.name, name.span),
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.diags.error(
                            target.span,
                            format!("assignment target must be a struct field (got {other})"),
                        );
                        Ty::Unknown
                    }
                }
            }
            ExprKind::Index { base, index } => {
                let base_ty = Self::unbox_transparent(self.check_expr(base));
                let index_ty = self.check_expr(index);
                match base_ty {
                    Ty::List(elem_ty) => {
                        if !matches!(index_ty, Ty::Unknown) && !self.is_int_like(&index_ty) {
                            self.diags.error(index.span, "list index must be Int");
                        }
                        *elem_ty
                    }
                    Ty::Map(key_ty, value_ty) => {
                        if !self.is_assignable(&index_ty, &key_ty) {
                            self.diags.error(index.span, "map index type mismatch");
                        }
                        *value_ty
                    }
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.diags.error(
                            target.span,
                            format!("assignment target must be an indexable value (got {other})"),
                        );
                        Ty::Unknown
                    }
                }
            }
            ExprKind::OptionalIndex { base, index } => {
                let base_ty = self.check_expr(base);
                let inner = match base_ty {
                    Ty::Option(inner) => *inner,
                    Ty::Unknown => return Ty::Unknown,
                    other => {
                        self.diags.error(
                            base.span,
                            format!("optional access on non-optional {}", other),
                        );
                        return Ty::Unknown;
                    }
                };
                let index_ty = self.check_expr(index);
                match inner {
                    Ty::List(elem_ty) => {
                        if !matches!(index_ty, Ty::Unknown) && !self.is_int_like(&index_ty) {
                            self.diags.error(index.span, "list index must be Int");
                        }
                        *elem_ty
                    }
                    Ty::Map(key_ty, value_ty) => {
                        if !self.is_assignable(&index_ty, &key_ty) {
                            self.diags.error(index.span, "map index type mismatch");
                        }
                        *value_ty
                    }
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        self.diags.error(
                            target.span,
                            format!("assignment target must be an indexable value (got {other})"),
                        );
                        Ty::Unknown
                    }
                }
            }
            _ => {
                self.diags.error(target.span, "invalid assignment target");
                Ty::Unknown
            }
        }
    }

    fn resolve_ident_expr(&mut self, ident: &crate::ast::Ident) -> Ty {
        if let Some(var) = self.env.lookup(&ident.name) {
            if self.in_spawn_scope() && matches!(var.ty, Ty::Boxed(_)) {
                self.diags
                    .error(ident.span, "spawn blocks cannot capture or use box values");
            }
            return Self::unbox_transparent(var.ty.clone());
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
        if self.symbols.types.contains_key(&ident.name)
            || self.symbols.enums.contains_key(&ident.name)
        {
            self.diags
                .error(ident.span, format!("{} is a type, not a value", ident.name));
            return Ty::Unknown;
        }
        if self.modules.contains(&ident.name) {
            self.diags.error(
                ident.span,
                format!("{} is a module, not a value", ident.name),
            );
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
                let base_ty = self.resolve_simple_type_name_in(module_id, &base.name, base.span);
                self.validate_refined_constraints(module_id, base, &base_ty, args);
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

    fn validate_refined_constraints(
        &mut self,
        module_id: ModuleId,
        base: &crate::ast::Ident,
        base_ty: &Ty,
        args: &[Expr],
    ) {
        let constraints = match parse_constraints(args) {
            Ok(items) => items,
            Err(err) => {
                let span = if err.span == Span::default() {
                    base.span
                } else {
                    err.span
                };
                self.diags.error(span, err.message);
                return;
            }
        };
        for constraint in constraints {
            match constraint {
                RefinementConstraint::Range { min, max, span } => {
                    self.validate_refined_range_constraint(&base.name, min, max, span);
                }
                RefinementConstraint::Regex { span, .. } => {
                    if !base_is_string_like(&base.name) {
                        self.diags.error(
                            span,
                            format!(
                                "regex() constraint is only supported for string-like refined bases, found {}",
                                base.name
                            ),
                        );
                    }
                }
                RefinementConstraint::Predicate { name, span } => {
                    self.validate_refined_predicate(module_id, &name, span, base_ty, &base.name);
                }
            }
        }
    }

    fn validate_refined_range_constraint(
        &mut self,
        base_name: &str,
        min: NumberLiteral,
        max: NumberLiteral,
        span: Span,
    ) {
        match base_name {
            "String" | "Id" | "Email" | "Bytes" => {
                if min.as_i64().is_none() || max.as_i64().is_none() {
                    self.diags
                        .error(span, "range bounds for string-like types must be integers");
                }
            }
            "Int" => {
                if min.as_i64().is_none() || max.as_i64().is_none() {
                    self.diags
                        .error(span, "range bounds for Int refinements must be integers");
                }
            }
            "Float" => {}
            _ => self.diags.error(
                span,
                format!(
                    "range constraints are not supported for refined base type {}",
                    base_name
                ),
            ),
        }
    }

    fn validate_refined_predicate(
        &mut self,
        module_id: ModuleId,
        fn_name: &str,
        span: Span,
        base_ty: &Ty,
        base_name: &str,
    ) {
        let sig = match self.resolve_function_sig_in_scope(module_id, fn_name) {
            Some(sig) => sig,
            None => {
                self.diags.error(
                    span,
                    format!(
                        "unknown predicate function {} in current module/import scope",
                        fn_name
                    ),
                );
                return;
            }
        };
        if sig.params.len() != 1 {
            self.diags.error(
                span,
                format!(
                    "predicate {} must accept exactly one parameter (found {})",
                    fn_name,
                    sig.params.len()
                ),
            );
            return;
        }
        let param_ty = &sig.params[0].ty;
        if !self.is_assignable(base_ty, param_ty) {
            self.diags.error(
                span,
                format!(
                    "predicate {} parameter type mismatch: expected {}, found {}",
                    fn_name, base_name, param_ty
                ),
            );
        }
        if !self.is_assignable(sig.ret.as_ref(), &Ty::Bool) {
            self.diags.error(
                span,
                format!(
                    "predicate {} must return Bool, found {}",
                    fn_name,
                    sig.ret.as_ref()
                ),
            );
        }
    }

    fn resolve_function_sig_in_scope(&mut self, module_id: ModuleId, name: &str) -> Option<FnSig> {
        if let Some(sig) = self.fn_sig_in(module_id, name) {
            return Some(sig);
        }
        let imported_module = self
            .module_import_items
            .get(&module_id)
            .and_then(|items| items.get(name))
            .map(|link| link.id)?;
        self.fn_sig_in(imported_module, name)
    }

    fn resolve_simple_type_name_in(&mut self, module_id: ModuleId, name: &str, span: Span) -> Ty {
        if !name.starts_with("std.") {
            if let Some((module_name, item_name)) = split_qualified_type_name(name) {
                let module_map = self.module_maps.get(&module_id).unwrap_or(self.modules);
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
                    self.diags
                        .error(span, format!("{}.{} is not a type", module_name, item_name));
                } else {
                    self.diags
                        .error(span, format!("unknown type {}.{}", module_name, item_name));
                }
                return Ty::Unknown;
            }
            if name.contains('.') {
                self.diags
                    .error(span, format!("invalid type path {}", name));
                return Ty::Unknown;
            }
        }
        match name {
            "Unit" => Ty::Unit,
            "Int" => Ty::Int,
            "Float" => Ty::Float,
            "Bool" => Ty::Bool,
            "String" => Ty::String,
            "Bytes" => Ty::Bytes,
            "Html" => Ty::Html,
            "Id" => Ty::Id,
            "Email" => Ty::Email,
            "Error" => Ty::Error,
            _ => {
                let symbols = self.module_symbols.get(&module_id).unwrap_or(self.symbols);
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
        let left = Self::unbox_transparent(left);
        let right = Self::unbox_transparent(right);
        match *op {
            Add | Sub | Mul | Div | Mod => {
                if self.is_numeric(&left) && self.is_numeric(&right) {
                    if matches!(left, Ty::Float) || matches!(right, Ty::Float) {
                        Ty::Float
                    } else {
                        Ty::Int
                    }
                } else if matches!(*op, Add)
                    && (matches!(left, Ty::String) || matches!(right, Ty::String))
                {
                    Ty::String
                } else if left.is_unknown() || right.is_unknown() {
                    Ty::Unknown
                } else {
                    self.diags
                        .error(span, "binary operator requires numeric types");
                    Ty::Unknown
                }
            }
            Eq | NotEq => {
                if !self.is_assignable(&left, &right) && !self.is_assignable(&right, &left) {
                    self.diags
                        .error(span, "equality comparison on incompatible types");
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
                    let elem_ty = self.range_elem_type(&left, &right);
                    Ty::List(Box::new(elem_ty))
                } else if left.is_unknown() || right.is_unknown() {
                    Ty::List(Box::new(Ty::Unknown))
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

    fn is_int_like(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Int => true,
            Ty::Refined { base, .. } => self.is_int_like(base),
            _ => false,
        }
    }

    fn range_elem_type(&self, left: &Ty, right: &Ty) -> Ty {
        let left_base = self.numeric_base_type(left);
        let right_base = self.numeric_base_type(right);
        match (left_base, right_base) {
            (Some(Ty::Unknown), _) | (_, Some(Ty::Unknown)) => Ty::Unknown,
            (Some(Ty::Float), _) | (_, Some(Ty::Float)) => Ty::Float,
            (Some(Ty::Int), Some(Ty::Int)) => Ty::Int,
            (Some(_), Some(_)) => Ty::Float,
            _ => Ty::Unknown,
        }
    }

    fn numeric_base_type(&self, ty: &Ty) -> Option<Ty> {
        match ty {
            Ty::Int => Some(Ty::Int),
            Ty::Float => Some(Ty::Float),
            Ty::Unknown => Some(Ty::Unknown),
            Ty::Refined { base, .. } => self.numeric_base_type(base),
            Ty::Boxed(inner) => self.numeric_base_type(inner),
            _ => None,
        }
    }

    fn is_assignable(&self, value: &Ty, target: &Ty) -> bool {
        if target == value {
            return true;
        }
        match (value, target) {
            (Ty::Boxed(value_inner), _) => self.is_assignable(value_inner, target),
            (_, Ty::Boxed(target_inner)) => self.is_assignable(value, target_inner),
            (Ty::Refined { base, .. }, _) => self.is_assignable(base, target),
            (Ty::Result(value_ok, value_err), Ty::Result(target_ok, target_err)) => {
                self.is_assignable(value_ok, target_ok) && self.is_assignable(value_err, target_err)
            }
            (_, Ty::Result(target_ok, _)) => self.is_assignable(value, target_ok),
            (Ty::Result(_, _), _) => false,
            (Ty::Option(value_inner), Ty::Option(target_inner)) => {
                self.is_assignable(value_inner, target_inner)
            }
            (Ty::List(value_inner), Ty::List(target_inner)) => {
                self.is_assignable(value_inner, target_inner)
            }
            (Ty::Map(value_key, value_val), Ty::Map(target_key, target_val)) => {
                self.is_assignable(value_key, target_key)
                    && self.is_assignable(value_val, target_val)
            }
            (Ty::Task(value_inner), Ty::Task(target_inner)) => {
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

    fn unbox_transparent(mut ty: Ty) -> Ty {
        while let Ty::Boxed(inner) = ty {
            ty = *inner;
        }
        ty
    }

    fn check_bang_error(&mut self, span: Span, err_ty: &Ty) {
        let expected_errs = match &self.current_return {
            Some(Ty::Result(_, _)) => {
                let mut errs = Vec::new();
                if let Some(current) = &self.current_return {
                    self.collect_result_errors(current, &mut errs);
                }
                if errs.is_empty() { None } else { Some(errs) }
            }
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
        if let Some(expected_errs) = expected_errs {
            if !expected_errs
                .iter()
                .any(|expected| self.is_assignable(err_ty, expected))
            {
                if let Some(first) = expected_errs.first() {
                    self.type_mismatch(span, first, err_ty);
                }
            }
        }
    }

    fn collect_result_errors(&self, ty: &Ty, out: &mut Vec<Ty>) {
        let mut current = ty;
        loop {
            let Ty::Result(_, err) = current else {
                break;
            };
            if let Ty::Result(ok_next, _) = &**err {
                out.push(*ok_next.clone());
                current = err;
                continue;
            }
            out.push(*err.clone());
            break;
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

    fn check_enum_variant_pattern(&mut self, ty: &Ty, name: &str, args: &[Pattern], span: Span) {
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
                        self.diags
                            .error(span, format!("unknown variant {} for {}", name, enum_name));
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
                    self.diags.error(route.path.span, "invalid route parameter");
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
                self.diags
                    .error(route.path.span, "unclosed route parameter");
                break;
            }
        }
        out
    }

    fn should_use_html_tag_builtin(&mut self, name: &str) -> bool {
        should_use_html_tag_builtin(
            name,
            name != "html" && self.env.lookup(name).is_some(),
            self.fn_sig_in(self.module_id, name).is_some(),
            self.config_info(name).is_some(),
            self.import_items.contains_key(name),
        )
    }

    fn check_html_tag_call(&mut self, span: Span, tag: &str, args: &[CallArg]) -> Ty {
        let Some(kind) = html_tags::tag_kind(tag) else {
            return Ty::Unknown;
        };
        let has_named = args.iter().any(|arg| arg.name.is_some());
        if has_named {
            for arg in args {
                let _ = self.check_expr(&arg.value);
            }
            if let Some(message) = self.validate_html_tag_named_args(args) {
                self.diags.error(span, message);
            }
            return Ty::Html;
        }

        let max = match kind {
            HtmlTagKind::Normal => 2usize,
            HtmlTagKind::Void => 1usize,
        };
        if args.len() > max {
            self.diags.error(
                span,
                format!("expected at most {} arguments, got {}", max, args.len()),
            );
        }
        if let Some(attrs) = args.get(0) {
            let attrs_ty = self.check_expr(&attrs.value);
            let expected_attrs = Ty::Map(Box::new(Ty::String), Box::new(Ty::String));
            if !self.is_assignable(&attrs_ty, &expected_attrs) {
                self.type_mismatch(attrs.span, &expected_attrs, &attrs_ty);
            }
        }
        if let Some(children) = args.get(1) {
            if matches!(kind, HtmlTagKind::Void) {
                self.diags.error(
                    children.span,
                    format!("void html tag {} does not accept children", tag),
                );
            }
            let children_ty = self.check_expr(&children.value);
            let expected_children = Ty::List(Box::new(Ty::Html));
            if !self.is_assignable(&children_ty, &expected_children) {
                self.type_mismatch(children.span, &expected_children, &children_ty);
            }
        }
        for arg in args.iter().skip(2) {
            let _ = self.check_expr(&arg.value);
        }
        Ty::Html
    }

    fn validate_html_tag_named_args(&self, args: &[CallArg]) -> Option<&'static str> {
        validate_named_args_for_phase(args, CanonicalizationPhase::TypeCheck)
    }
}

fn lvalue_root_name(target: &Expr) -> Option<&str> {
    match &target.kind {
        ExprKind::Ident(ident) => Some(ident.name.as_str()),
        ExprKind::Member { base, .. }
        | ExprKind::OptionalMember { base, .. }
        | ExprKind::Index { base, .. }
        | ExprKind::OptionalIndex { base, .. } => lvalue_root_name(base),
        _ => None,
    }
}

fn force_html_input_tag_call(name: &str, args: &[CallArg]) -> bool {
    if name != "input" {
        return false;
    }
    args.iter()
        .any(|arg| arg.name.is_some() || arg.is_block_sugar)
        || matches!(
            args.first().map(|arg| &arg.value.kind),
            Some(ExprKind::MapLit(_))
        )
}

fn capability_for_ident_call(name: &str) -> Option<Capability> {
    match name {
        "serve" => Some(Capability::Network),
        "time" => Some(Capability::Time),
        _ => None,
    }
}

fn spawn_forbidden_builtin(callee: &Expr) -> Option<&'static str> {
    match &callee.kind {
        ExprKind::Ident(ident) => match ident.name.as_str() {
            "print" => Some("print"),
            "input" => Some("input"),
            "log" => Some("log"),
            "env" => Some("env"),
            "asset" => Some("asset"),
            "serve" => Some("serve"),
            _ => None,
        },
        ExprKind::Member { base, name } => match &base.kind {
            ExprKind::Ident(ident) if ident.name == "db" => Some("db.*"),
            ExprKind::Ident(ident) if ident.name == "svg" && name.name == "inline" => {
                Some("svg.inline")
            }
            _ => None,
        },
        _ => None,
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

    fn depth(&self) -> usize {
        self.scopes.len()
    }

    fn insert_builtin(&mut self, name: &str) {
        let _ = self.insert(name, Ty::Unknown, false);
    }

    fn insert_builtin_with_ty(&mut self, name: &str, ty: Ty) {
        let _ = self.insert(name, ty, false);
    }

    fn insert(&mut self, name: &str, ty: Ty, mutable: bool) -> Result<(), String> {
        if let Some(scope) = self.scopes.last_mut() {
            if scope.vars.contains_key(name) {
                return Err(format!("duplicate binding: {name}"));
            }
            scope.vars.insert(name.to_string(), VarInfo { ty, mutable });
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

    fn lookup_with_depth(&self, name: &str) -> Option<(&VarInfo, usize)> {
        for (idx, scope) in self.scopes.iter().enumerate().rev() {
            if let Some(var) = scope.vars.get(name) {
                return Some((var, idx));
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
