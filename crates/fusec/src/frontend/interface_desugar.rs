use std::collections::HashMap;

use crate::ast::{
    AppDecl, Block, CallArg, ComponentDecl, ConfigDecl, Expr, ExprKind, FnDecl, Ident, ImplDecl,
    InterpPart, Item, Literal, MigrationDecl, Param, Pattern, PatternKind, Program, RouteDecl,
    ServiceDecl, Stmt, StmtKind, TestDecl, TypeRef, TypeRefKind,
};
use crate::diag::Diagnostics;
use crate::loader::{ModuleId, ModuleLink, ModuleMap, ModuleRegistry};
use crate::sema::symbols::{self, FnSigRef, ModuleSymbols};
use crate::sema::types::{FnSig, ParamSig, Ty};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum InterfaceMethodKind {
    Instance,
    Associated,
}

#[derive(Clone, Debug)]
struct SyntheticMethod {
    owner_module: ModuleId,
    target: String,
    raw_name: String,
    kind: InterfaceMethodKind,
    sig: FnSig,
}

#[derive(Default)]
struct SyntheticIndex {
    dispatch: HashMap<(String, String, InterfaceMethodKind), Vec<SyntheticMethod>>,
    methods_by_owner: HashMap<(ModuleId, String, String, String), SyntheticMethod>,
    function_sigs: HashMap<(ModuleId, String), FnSig>,
    function_self_types: HashMap<(ModuleId, String), Ty>,
}

impl SyntheticIndex {
    fn build(
        registry: &ModuleRegistry,
        symbols_by_id: &HashMap<ModuleId, ModuleSymbols>,
    ) -> Self {
        let mut index = Self::default();
        let mut ids: Vec<_> = registry.modules.keys().copied().collect();
        ids.sort_unstable();
        for module_id in ids {
            let Some(unit) = registry.modules.get(&module_id) else {
                continue;
            };
            for item in &unit.program.items {
                let Item::Impl(decl) = item else {
                    continue;
                };
                let Some(target_ty) = resolve_nominal_type_in_scope(
                    module_id,
                    &decl.target.name,
                    &unit.modules,
                    &unit.import_items,
                    symbols_by_id,
                ) else {
                    continue;
                };
                let target_name = match &target_ty {
                    Ty::Struct(name) | Ty::Enum(name) => name.clone(),
                    _ => continue,
                };
                for method in &decl.methods {
                    let uses_self = method_uses_self(module_id, decl, method, symbols_by_id);
                    let kind = if uses_self {
                        InterfaceMethodKind::Instance
                    } else {
                        InterfaceMethodKind::Associated
                    };
                    let raw_name = synthetic_method_name(
                        &decl.interface.name,
                        &target_name,
                        &method.name.name,
                        kind,
                    );
                    let sig = synth_method_sig(module_id, method, &target_ty, kind, unit, symbols_by_id);
                    let synth = SyntheticMethod {
                        owner_module: module_id,
                        target: target_name.clone(),
                        raw_name: raw_name.clone(),
                        kind,
                        sig: sig.clone(),
                    };
                    index
                        .dispatch
                        .entry((target_name.clone(), method.name.name.clone(), kind))
                        .or_default()
                        .push(synth.clone());
                    index.methods_by_owner.insert(
                        (
                            module_id,
                            decl.interface.name.clone(),
                            target_name.clone(),
                            method.name.name.clone(),
                        ),
                        synth.clone(),
                    );
                    index
                        .function_sigs
                        .insert((module_id, raw_name.clone()), sig);
                    index
                        .function_self_types
                        .insert((module_id, raw_name), target_ty.clone());
                }
            }
        }
        index
    }

    fn resolve(
        &self,
        target: &str,
        method: &str,
        kind: InterfaceMethodKind,
    ) -> Option<&SyntheticMethod> {
        let candidates = self
            .dispatch
            .get(&(target.to_string(), method.to_string(), kind))?;
        if candidates.len() == 1 {
            candidates.first()
        } else {
            None
        }
    }

    fn function_sig(&self, module_id: ModuleId, name: &str) -> Option<FnSig> {
        self.function_sigs
            .get(&(module_id, name.to_string()))
            .cloned()
    }

    fn function_self_ty(&self, module_id: ModuleId, name: &str) -> Option<Ty> {
        self.function_self_types
            .get(&(module_id, name.to_string()))
            .cloned()
    }
}

#[derive(Default)]
struct GlobalSymbols {
    types: HashMap<String, symbols::TypeInfo>,
    configs: HashMap<String, symbols::ConfigInfo>,
    enums: HashMap<String, symbols::EnumInfo>,
}

impl GlobalSymbols {
    fn build(symbols_by_id: &HashMap<ModuleId, ModuleSymbols>) -> Self {
        let mut global = Self::default();
        let mut ids: Vec<_> = symbols_by_id.keys().copied().collect();
        ids.sort_unstable();
        for module_id in ids {
            let Some(symbols) = symbols_by_id.get(&module_id) else {
                continue;
            };
            for (name, info) in &symbols.types {
                global.types.entry(name.clone()).or_insert_with(|| info.clone());
            }
            for (name, info) in &symbols.configs {
                global
                    .configs
                    .entry(name.clone())
                    .or_insert_with(|| info.clone());
            }
            for (name, info) in &symbols.enums {
                global.enums.entry(name.clone()).or_insert_with(|| info.clone());
            }
        }
        global
    }
}

pub fn desugar_registry(registry: &ModuleRegistry) -> ModuleRegistry {
    let mut diags = Diagnostics::default();
    let mut symbols_by_id = HashMap::new();
    let mut ids: Vec<_> = registry.modules.keys().copied().collect();
    ids.sort_unstable();
    for module_id in &ids {
        if let Some(unit) = registry.modules.get(module_id) {
            symbols_by_id.insert(*module_id, symbols::collect(&unit.program, &mut diags));
        }
    }
    let synth_index = SyntheticIndex::build(registry, &symbols_by_id);
    let global = GlobalSymbols::build(&symbols_by_id);
    let mut lowered = registry.clone();

    for module_id in &ids {
        let Some(source_unit) = registry.modules.get(module_id) else {
            continue;
        };
        let mut synthetic_items = Vec::new();
        for item in &source_unit.program.items {
            let Item::Impl(decl) = item else {
                continue;
            };
            let Some(target_ty) = resolve_nominal_type_in_scope(
                *module_id,
                &decl.target.name,
                &source_unit.modules,
                &source_unit.import_items,
                &symbols_by_id,
            ) else {
                continue;
            };
            let target_name = match &target_ty {
                Ty::Struct(name) | Ty::Enum(name) => name.clone(),
                _ => continue,
            };
            for method in &decl.methods {
                let Some(synth) = synth_index.methods_by_owner.get(&(
                    *module_id,
                    decl.interface.name.clone(),
                    target_name.clone(),
                    method.name.name.clone(),
                )) else {
                    continue;
                };
                synthetic_items.push(Item::Fn(synthesize_method_decl(method, synth)));
            }
        }
        if let Some(unit) = lowered.modules.get_mut(module_id) {
            unit.program.items.extend(synthetic_items);
        }
    }

    for module_id in ids {
        let Some(unit) = lowered.modules.get_mut(&module_id) else {
            continue;
        };
        let mut rewriter = InterfaceRewriter::new(
            module_id,
            &unit.modules,
            &unit.import_items,
            symbols_by_id.get(&module_id),
            &symbols_by_id,
            &global,
            &synth_index,
        );
        rewriter.rewrite_program(&mut unit.program);
    }

    lowered
}

struct InterfaceRewriter<'a> {
    module_id: ModuleId,
    modules: &'a ModuleMap,
    import_items: &'a HashMap<String, ModuleLink>,
    local_symbols: Option<&'a ModuleSymbols>,
    symbols_by_id: &'a HashMap<ModuleId, ModuleSymbols>,
    global: &'a GlobalSymbols,
    synth_index: &'a SyntheticIndex,
    scopes: Vec<HashMap<String, Ty>>,
    current_self_type: Option<Ty>,
}

impl<'a> InterfaceRewriter<'a> {
    fn new(
        module_id: ModuleId,
        modules: &'a ModuleMap,
        import_items: &'a HashMap<String, ModuleLink>,
        local_symbols: Option<&'a ModuleSymbols>,
        symbols_by_id: &'a HashMap<ModuleId, ModuleSymbols>,
        global: &'a GlobalSymbols,
        synth_index: &'a SyntheticIndex,
    ) -> Self {
        Self {
            module_id,
            modules,
            import_items,
            local_symbols,
            symbols_by_id,
            global,
            synth_index,
            scopes: vec![HashMap::new()],
            current_self_type: None,
        }
    }

    fn rewrite_program(&mut self, program: &mut Program) {
        for item in &mut program.items {
            match item {
                Item::Type(decl) => self.rewrite_type_decl(decl),
                Item::Enum(_) | Item::Import(_) | Item::Interface(_) => {}
                Item::Impl(decl) => self.rewrite_impl_decl(decl),
                Item::Fn(decl) => {
                    let self_ty = self
                        .synth_index
                        .function_self_ty(self.module_id, &decl.name.name);
                    self.rewrite_fn_decl(decl, self_ty, false);
                }
                Item::Component(decl) => self.rewrite_component_decl(decl),
                Item::Service(decl) => self.rewrite_service_decl(decl),
                Item::Config(decl) => self.rewrite_config_decl(decl),
                Item::App(decl) => self.rewrite_app_decl(decl),
                Item::Migration(decl) => self.rewrite_migration_decl(decl),
                Item::Test(decl) => self.rewrite_test_decl(decl),
            }
        }
    }

    fn rewrite_type_decl(&mut self, decl: &mut crate::ast::TypeDecl) {
        for field in &mut decl.fields {
            if let Some(default) = &mut field.default {
                self.rewrite_expr(default);
            }
        }
    }

    fn rewrite_impl_decl(&mut self, decl: &mut ImplDecl) {
        let current_self = resolve_nominal_type_in_scope(
            self.module_id,
            &decl.target.name,
            self.modules,
            self.import_items,
            self.symbols_by_id,
        );
        for method in &mut decl.methods {
            self.rewrite_fn_decl(method, current_self.clone(), true);
        }
    }

    fn rewrite_component_decl(&mut self, decl: &mut ComponentDecl) {
        self.scopes.push(HashMap::new());
        self.bind_local(
            "attrs",
            Ty::Map(Box::new(Ty::String), Box::new(Ty::String)),
        );
        self.bind_local("children", Ty::List(Box::new(Ty::Html)));
        self.rewrite_block(&mut decl.body);
        self.scopes.pop();
    }

    fn rewrite_service_decl(&mut self, decl: &mut ServiceDecl) {
        for route in &mut decl.routes {
            self.rewrite_route_decl(route);
        }
    }

    fn rewrite_config_decl(&mut self, decl: &mut ConfigDecl) {
        for field in &mut decl.fields {
            self.rewrite_expr(&mut field.value);
        }
    }

    fn rewrite_app_decl(&mut self, decl: &mut AppDecl) {
        self.rewrite_block(&mut decl.body);
    }

    fn rewrite_migration_decl(&mut self, decl: &mut MigrationDecl) {
        self.rewrite_block(&mut decl.body);
    }

    fn rewrite_test_decl(&mut self, decl: &mut TestDecl) {
        self.rewrite_block(&mut decl.body);
    }

    fn rewrite_route_decl(&mut self, route: &mut RouteDecl) {
        self.rewrite_block(&mut route.body);
    }

    fn rewrite_fn_decl(&mut self, decl: &mut FnDecl, current_self: Option<Ty>, bind_self: bool) {
        let prev_self = self.current_self_type.clone();
        self.current_self_type = current_self.clone();
        self.scopes.push(HashMap::new());
        if bind_self {
            if let Some(self_ty) = &current_self {
                self.bind_local("self", self_ty.clone());
            }
        }
        for param in &mut decl.params {
            if let Some(default) = &mut param.default {
                self.rewrite_expr(default);
            }
            let param_ty = self.resolve_type_ref(&param.ty);
            self.bind_local(&param.name.name, param_ty);
        }
        self.rewrite_block(&mut decl.body);
        self.scopes.pop();
        self.current_self_type = prev_self;
    }

    fn rewrite_block(&mut self, block: &mut Block) {
        self.scopes.push(HashMap::new());
        for stmt in &mut block.stmts {
            self.rewrite_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn rewrite_stmt(&mut self, stmt: &mut Stmt) {
        match &mut stmt.kind {
            StmtKind::Let { name, ty, expr } | StmtKind::Var { name, ty, expr } => {
                self.rewrite_expr(expr);
                let value_ty = match ty {
                    Some(ty) => self.resolve_type_ref(ty),
                    None => self.expr_ty(expr),
                };
                self.bind_local(&name.name, value_ty);
            }
            StmtKind::Assign { target, expr } => {
                self.rewrite_expr(target);
                self.rewrite_expr(expr);
            }
            StmtKind::Return { expr } => {
                if let Some(expr) = expr {
                    self.rewrite_expr(expr);
                }
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                self.rewrite_expr(cond);
                self.rewrite_block(then_block);
                for (branch_cond, branch_block) in else_if {
                    self.rewrite_expr(branch_cond);
                    self.rewrite_block(branch_block);
                }
                if let Some(block) = else_block {
                    self.rewrite_block(block);
                }
            }
            StmtKind::Match { expr, cases } => {
                self.rewrite_expr(expr);
                let expr_ty = self.expr_ty(expr);
                for (pattern, block) in cases {
                    self.rewrite_pattern(pattern);
                    self.scopes.push(HashMap::new());
                    self.bind_pattern(pattern, &expr_ty);
                    self.rewrite_block(block);
                    self.scopes.pop();
                }
            }
            StmtKind::For { pat, iter, block } => {
                self.rewrite_expr(iter);
                self.rewrite_pattern(pat);
                let item_ty = match self.expr_ty(iter) {
                    Ty::List(inner) => *inner,
                    Ty::Map(_, value) => *value,
                    _ => Ty::Unknown,
                };
                self.scopes.push(HashMap::new());
                self.bind_pattern(pat, &item_ty);
                self.rewrite_block(block);
                self.scopes.pop();
            }
            StmtKind::While { cond, block } => {
                self.rewrite_expr(cond);
                self.rewrite_block(block);
            }
            StmtKind::Transaction { block } => self.rewrite_block(block),
            StmtKind::Expr(expr) => {
                self.rewrite_expr(expr);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn rewrite_pattern(&mut self, pattern: &mut Pattern) {
        match &mut pattern.kind {
            PatternKind::Wildcard | PatternKind::Literal(_) | PatternKind::Ident(_) => {}
            PatternKind::EnumVariant { args, .. } => {
                for arg in args {
                    self.rewrite_pattern(arg);
                }
            }
            PatternKind::Struct { name, fields } => {
                if name.name == "Self" {
                    if let Some(target_name) = self.current_self_name() {
                        name.name = target_name;
                    }
                }
                for field in fields {
                    self.rewrite_pattern(&mut field.pat);
                }
            }
        }
    }

    fn rewrite_expr(&mut self, expr: &mut Expr) -> Ty {
        match &mut expr.kind {
            ExprKind::Literal(_) | ExprKind::Ident(_) => {}
            ExprKind::Binary { left, right, .. } | ExprKind::Coalesce { left, right } => {
                self.rewrite_expr(left);
                self.rewrite_expr(right);
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::Await { expr }
            | ExprKind::Box { expr }
            | ExprKind::BangChain { expr, error: None } => {
                self.rewrite_expr(expr);
            }
            ExprKind::BangChain {
                expr,
                error: Some(error),
            } => {
                self.rewrite_expr(expr);
                self.rewrite_expr(error);
            }
            ExprKind::Call {
                callee,
                args,
                type_args: _,
            } => {
                self.rewrite_expr(callee);
                for arg in args {
                    self.rewrite_expr(&mut arg.value);
                }
            }
            ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
                self.rewrite_expr(base);
            }
            ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
                self.rewrite_expr(base);
                self.rewrite_expr(index);
            }
            ExprKind::StructLit { name, fields } => {
                if name.name == "Self" {
                    if let Some(target_name) = self.current_self_name() {
                        name.name = target_name;
                    }
                }
                for field in fields {
                    self.rewrite_expr(&mut field.value);
                }
            }
            ExprKind::ListLit(items) => {
                for item in items {
                    self.rewrite_expr(item);
                }
            }
            ExprKind::MapLit(pairs) => {
                for (key, value) in pairs {
                    self.rewrite_expr(key);
                    self.rewrite_expr(value);
                }
            }
            ExprKind::InterpString(parts) => {
                for part in parts {
                    if let InterpPart::Expr(expr) = part {
                        self.rewrite_expr(expr);
                    }
                }
            }
            ExprKind::Spawn { block } => self.rewrite_block(block),
            ExprKind::HtmlIf {
                cond,
                then_children,
                else_if,
                else_children,
            } => {
                self.rewrite_expr(cond);
                for child in then_children {
                    self.rewrite_expr(child);
                }
                for (branch_cond, branch_children) in else_if {
                    self.rewrite_expr(branch_cond);
                    for child in branch_children {
                        self.rewrite_expr(child);
                    }
                }
                for child in else_children {
                    self.rewrite_expr(child);
                }
            }
            ExprKind::HtmlFor {
                pat,
                iter,
                body_children,
            } => {
                self.rewrite_expr(iter);
                self.rewrite_pattern(pat);
                let item_ty = match self.expr_ty(iter) {
                    Ty::List(inner) => *inner,
                    Ty::Map(_, value) => *value,
                    _ => Ty::Unknown,
                };
                self.scopes.push(HashMap::new());
                self.bind_pattern(pat, &item_ty);
                for child in body_children {
                    self.rewrite_expr(child);
                }
                self.scopes.pop();
            }
        }

        if let Some(ty) = self.try_rewrite_call(expr) {
            return ty;
        }
        if let Some(ty) = self.try_rewrite_member(expr) {
            return ty;
        }
        self.expr_ty(expr)
    }

    fn try_rewrite_call(&self, expr: &mut Expr) -> Option<Ty> {
        let ExprKind::Call {
            callee,
            args,
            type_args,
        } = &expr.kind
        else {
            return None;
        };
        if !type_args.is_empty() {
            return None;
        }
        let ExprKind::Member { base, name } = &callee.kind else {
            return None;
        };
        let resolution = self.resolve_member(base, name, false);
        let synth = resolution.synthetic?;
        let mut new_args = Vec::with_capacity(args.len() + usize::from(matches!(synth.kind, InterfaceMethodKind::Instance)));
        if matches!(synth.kind, InterfaceMethodKind::Instance) {
            new_args.push(CallArg {
                name: None,
                value: (*base.clone()),
                span: base.span,
                comma_before: None,
                is_block_sugar: false,
            });
        }
        new_args.extend(args.iter().cloned());
        expr.kind = ExprKind::Call {
            callee: Box::new(Expr {
                kind: ExprKind::Ident(Ident {
                    name: canonical_internal_function_name(synth.owner_module, &synth.raw_name),
                    span: callee.span,
                }),
                span: callee.span,
            }),
            args: new_args,
            type_args: Vec::new(),
        };
        Some(*synth.sig.ret.clone())
    }

    fn try_rewrite_member(&self, expr: &mut Expr) -> Option<Ty> {
        let ExprKind::Member { base, name } = &expr.kind else {
            return None;
        };
        let resolution = self.resolve_member(base, name, false);
        let synth = resolution.synthetic?;
        if !matches!(synth.kind, InterfaceMethodKind::Associated) {
            return None;
        }
        expr.kind = ExprKind::Ident(Ident {
            name: canonical_internal_function_name(synth.owner_module, &synth.raw_name),
            span: expr.span,
        });
        Some(Ty::Fn(synth.sig.clone()))
    }

    fn expr_ty(&self, expr: &Expr) -> Ty {
        match &expr.kind {
            ExprKind::Literal(lit) => self.ty_from_literal(lit),
            ExprKind::Ident(ident) => self.resolve_ident_ty(&ident.name),
            ExprKind::Binary { op, left, right } => self.binary_ty(op, &self.expr_ty(left), &self.expr_ty(right)),
            ExprKind::Unary { expr, .. } => self.expr_ty(expr),
            ExprKind::Call {
                callee,
                args: _,
                type_args: _,
            } => match self.expr_ty(callee) {
                Ty::Fn(sig) => *sig.ret,
                Ty::External(name) if name == "query.one" => Ty::Option(Box::new(Ty::Map(Box::new(Ty::String), Box::new(Ty::Unknown)))),
                Ty::External(name) if name == "query.all" => Ty::List(Box::new(Ty::Map(Box::new(Ty::String), Box::new(Ty::Unknown)))),
                _ => Ty::Unknown,
            },
            ExprKind::Member { base, name } => self.resolve_member(base, name, false).ty,
            ExprKind::OptionalMember { base, name } => self.resolve_member(base, name, true).ty,
            ExprKind::Index { base, .. } => match self.expr_ty(base) {
                Ty::List(inner) => *inner,
                Ty::Map(_, value) => *value,
                _ => Ty::Unknown,
            },
            ExprKind::OptionalIndex { base, .. } => match self.expr_ty(base) {
                Ty::Option(inner) => match *inner {
                    Ty::List(inner) => Ty::Option(inner),
                    Ty::Map(_, value) => Ty::Option(value),
                    _ => Ty::Option(Box::new(Ty::Unknown)),
                },
                _ => Ty::Option(Box::new(Ty::Unknown)),
            },
            ExprKind::StructLit { name, .. } => self.resolve_struct_lit_ty(&name.name),
            ExprKind::ListLit(items) => {
                let inner = items.first().map(|item| self.expr_ty(item)).unwrap_or(Ty::Unknown);
                Ty::List(Box::new(inner))
            }
            ExprKind::MapLit(items) => {
                let value_ty = items
                    .first()
                    .map(|(_, value)| self.expr_ty(value))
                    .unwrap_or(Ty::Unknown);
                Ty::Map(Box::new(Ty::String), Box::new(value_ty))
            }
            ExprKind::InterpString(_) => Ty::String,
            ExprKind::Coalesce { left, right } => match self.expr_ty(left) {
                Ty::Option(inner) => self.unify_types(*inner, self.expr_ty(right)),
                left_ty => self.unify_types(left_ty, self.expr_ty(right)),
            },
            ExprKind::BangChain { expr, .. } => match self.expr_ty(expr) {
                Ty::Option(inner) => *inner,
                Ty::Result(ok, _) => *ok,
                other => other,
            },
            ExprKind::Spawn { block } => Ty::Task(Box::new(self.block_ty(block))),
            ExprKind::HtmlIf { .. } | ExprKind::HtmlFor { .. } => Ty::List(Box::new(Ty::Html)),
            ExprKind::Await { expr } => match self.expr_ty(expr) {
                Ty::Task(inner) => *inner,
                other => other,
            },
            ExprKind::Box { expr } => Ty::Boxed(Box::new(self.expr_ty(expr))),
        }
    }

    fn block_ty(&self, block: &Block) -> Ty {
        block
            .stmts
            .last()
            .map(|stmt| self.stmt_ty(stmt))
            .unwrap_or(Ty::Unit)
    }

    fn stmt_ty(&self, stmt: &Stmt) -> Ty {
        match &stmt.kind {
            StmtKind::Expr(expr) => self.expr_ty(expr),
            StmtKind::Return { expr } => expr.as_ref().map(|expr| self.expr_ty(expr)).unwrap_or(Ty::Unit),
            _ => Ty::Unit,
        }
    }

    fn resolve_ident_ty(&self, name: &str) -> Ty {
        if let Some(ty) = self.lookup_local(name) {
            return ty;
        }
        if let Some((module_id, raw_name)) = parse_canonical_function_name(name) {
            if let Some(sig) = self.function_sig(module_id, raw_name) {
                return Ty::Fn(sig);
            }
        }
        if let Some(sig) = self.function_sig(self.module_id, name) {
            return Ty::Fn(sig);
        }
        if let Some(link) = self.import_items.get(name) {
            if let Some(sig) = self.function_sig(link.id, name) {
                return Ty::Fn(sig);
            }
            if self.symbols_in(link.id).configs.contains_key(name) {
                return Ty::Config(name.to_string());
            }
        }
        if self.local_symbols
            .is_some_and(|symbols| symbols.configs.contains_key(name))
        {
            return Ty::Config(name.to_string());
        }
        if self.modules.contains(name) {
            return Ty::Module(name.to_string());
        }
        match name {
            "db" | "json" | "html" | "svg" | "request" | "response" | "http" | "time" | "crypto" => {
                Ty::External(name.to_string())
            }
            _ => Ty::Unknown,
        }
    }

    fn resolve_member(&self, base: &Expr, name: &Ident, is_optional: bool) -> MemberResolution {
        let (base_ty, associated_receiver) = self.member_base(base);
        let mut inner = self.unbox_transparent(base_ty);
        if is_optional {
            inner = match inner {
                Ty::Option(inner) => *inner,
                other => other,
            };
        }
        let ty = match &inner {
            Ty::Struct(type_name) => {
                if associated_receiver {
                    if let Some(synth) =
                        self.synth_index
                            .resolve(type_name, &name.name, InterfaceMethodKind::Associated)
                    {
                        Ty::Fn(synth.sig.clone())
                    } else {
                        Ty::Unknown
                    }
                } else if let Some(field_ty) = self.lookup_type_field(type_name, &name.name) {
                    self.resolve_type_ref(&field_ty)
                } else if let Some(synth) =
                    self.synth_index
                        .resolve(type_name, &name.name, InterfaceMethodKind::Instance)
                {
                    Ty::Fn(synth.sig.clone())
                } else {
                    Ty::Unknown
                }
            }
            Ty::Enum(enum_name) => {
                if associated_receiver {
                    if let Some(variant) = self.lookup_enum_variant(enum_name, &name.name) {
                        Ty::Fn(enum_variant_sig(enum_name, &variant))
                    } else if let Some(synth) =
                        self.synth_index
                            .resolve(enum_name, &name.name, InterfaceMethodKind::Associated)
                    {
                        Ty::Fn(synth.sig.clone())
                    } else {
                        Ty::Unknown
                    }
                } else if let Some(synth) =
                    self.synth_index
                        .resolve(enum_name, &name.name, InterfaceMethodKind::Instance)
                {
                    Ty::Fn(synth.sig.clone())
                } else {
                    Ty::Unknown
                }
            }
            Ty::Config(config_name) => self
                .lookup_config_field(config_name, &name.name)
                .map(|ty| self.resolve_type_ref(&ty))
                .unwrap_or(Ty::Unknown),
            Ty::Module(module_name) => self.lookup_module_member(module_name, &name.name),
            Ty::External(external) => self.lookup_external_member(external, &name.name),
            other => other.clone(),
        };
        let synthetic = match &inner {
            Ty::Struct(type_name) | Ty::Enum(type_name) if associated_receiver => self
                .synth_index
                .resolve(type_name, &name.name, InterfaceMethodKind::Associated)
                .cloned(),
            Ty::Struct(type_name) | Ty::Enum(type_name) => self
                .synth_index
                .resolve(type_name, &name.name, InterfaceMethodKind::Instance)
                .cloned(),
            _ => None,
        };
        let ty = if is_optional {
            Ty::Option(Box::new(ty))
        } else {
            ty
        };
        MemberResolution { ty, synthetic }
    }

    fn member_base(&self, base: &Expr) -> (Ty, bool) {
        match &base.kind {
            ExprKind::Ident(ident) => {
                if let Some(ty) = self.lookup_local(&ident.name) {
                    (ty, false)
                } else if self.modules.contains(&ident.name) {
                    (Ty::Module(ident.name.clone()), false)
                } else if ident.name == "Self" {
                    if let Some(self_ty) = &self.current_self_type {
                        (self_ty.clone(), true)
                    } else {
                        (self.resolve_ident_ty(&ident.name), false)
                    }
                } else if let Some(ty) = self.lookup_nominal_type_in_scope(&ident.name) {
                    (ty, true)
                } else {
                    (self.resolve_ident_ty(&ident.name), false)
                }
            }
            ExprKind::Member {
                base: inner_base,
                name: inner_name,
            } => {
                if let ExprKind::Ident(module_ident) = &inner_base.kind {
                    let qualified = format!("{}.{}", module_ident.name, inner_name.name);
                    if let Some(ty) = self.lookup_nominal_type_in_scope(&qualified) {
                        return (ty, true);
                    }
                }
                (self.expr_ty(base), false)
            }
            _ => (self.expr_ty(base), false),
        }
    }

    fn lookup_module_member(&self, module_name: &str, member: &str) -> Ty {
        let Some(link) = self.modules.get(module_name) else {
            return Ty::Unknown;
        };
        if let Some(sig) = self.function_sig(link.id, member) {
            return Ty::Fn(sig);
        }
        if self.symbols_in(link.id).configs.contains_key(member) {
            return Ty::Config(member.to_string());
        }
        Ty::Unknown
    }

    fn lookup_external_member(&self, external: &str, member: &str) -> Ty {
        match (external, member) {
            ("db", "exec") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: vec![ParamSig {
                    name: "sql".to_string(),
                    ty: Ty::String,
                    has_default: false,
                }],
                ret: Box::new(Ty::Unit),
            }),
            ("db", "query") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: vec![ParamSig {
                    name: "sql".to_string(),
                    ty: Ty::String,
                    has_default: false,
                }],
                ret: Box::new(Ty::List(Box::new(Ty::Map(
                    Box::new(Ty::String),
                    Box::new(Ty::Unknown),
                )))),
            }),
            ("db", "one") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: vec![ParamSig {
                    name: "sql".to_string(),
                    ty: Ty::String,
                    has_default: false,
                }],
                ret: Box::new(Ty::Option(Box::new(Ty::Map(
                    Box::new(Ty::String),
                    Box::new(Ty::Unknown),
                )))),
            }),
            ("db", "from") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: vec![ParamSig {
                    name: "table".to_string(),
                    ty: Ty::String,
                    has_default: false,
                }],
                ret: Box::new(Ty::External("query".to_string())),
            }),
            ("query", method)
                if matches!(
                    method,
                    "select"
                        | "where"
                        | "order_by"
                        | "limit"
                        | "insert"
                        | "upsert"
                        | "update"
                        | "delete"
                ) => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: Vec::new(),
                ret: Box::new(Ty::External("query".to_string())),
            }),
            ("query", "count") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: Vec::new(),
                ret: Box::new(Ty::Int),
            }),
            ("query", "one") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: Vec::new(),
                ret: Box::new(Ty::Option(Box::new(Ty::Map(
                    Box::new(Ty::String),
                    Box::new(Ty::Unknown),
                )))),
            }),
            ("query", "all") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: Vec::new(),
                ret: Box::new(Ty::List(Box::new(Ty::Map(
                    Box::new(Ty::String),
                    Box::new(Ty::Unknown),
                )))),
            }),
            ("query", "exec") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: Vec::new(),
                ret: Box::new(Ty::Unit),
            }),
            ("query", "sql") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: Vec::new(),
                ret: Box::new(Ty::String),
            }),
            ("query", "params") => Ty::Fn(FnSig {
                type_params: Vec::new(),
                params: Vec::new(),
                ret: Box::new(Ty::List(Box::new(Ty::Unknown))),
            }),
            _ => Ty::Unknown,
        }
    }

    fn function_sig(&self, module_id: ModuleId, name: &str) -> Option<FnSig> {
        if let Some(sig) = self.synth_index.function_sig(module_id, name) {
            return Some(sig);
        }
        let symbols = self.symbols_in(module_id);
        let decl = symbols.functions.get(name)?;
        Some(self.resolve_fn_sig(module_id, decl))
    }

    fn resolve_fn_sig(&self, module_id: ModuleId, decl: &FnSigRef) -> FnSig {
        let params = decl
            .params
            .iter()
            .map(|param| ParamSig {
                name: param.name.clone(),
                ty: self.resolve_type_ref_in(module_id, &param.ty),
                has_default: param.has_default,
            })
            .collect();
        let ret = decl
            .ret
            .as_ref()
            .map(|ty| self.resolve_type_ref_in(module_id, ty))
            .unwrap_or(Ty::Unit);
        FnSig {
            type_params: Vec::new(),
            params,
            ret: Box::new(ret),
        }
    }

    fn resolve_type_ref(&self, ty: &TypeRef) -> Ty {
        self.resolve_type_ref_in(self.module_id, ty)
    }

    fn resolve_type_ref_in(&self, module_id: ModuleId, ty: &TypeRef) -> Ty {
        match &ty.kind {
            TypeRefKind::Simple(ident) => self.resolve_simple_type_name(module_id, &ident.name),
            TypeRefKind::Generic { base, args } => match base.name.as_str() {
                "List" => args
                    .first()
                    .map(|arg| Ty::List(Box::new(self.resolve_type_ref_in(module_id, arg))))
                    .unwrap_or(Ty::Unknown),
                "Map" => {
                    if args.len() != 2 {
                        return Ty::Unknown;
                    }
                    let key = self.resolve_type_ref_in(module_id, &args[0]);
                    let value = self.resolve_type_ref_in(module_id, &args[1]);
                    Ty::Map(Box::new(key), Box::new(value))
                }
                "Option" => args
                    .first()
                    .map(|arg| Ty::Option(Box::new(self.resolve_type_ref_in(module_id, arg))))
                    .unwrap_or(Ty::Unknown),
                "Result" => {
                    if args.len() != 2 {
                        return Ty::Unknown;
                    }
                    let ok = self.resolve_type_ref_in(module_id, &args[0]);
                    let err = self.resolve_type_ref_in(module_id, &args[1]);
                    Ty::Result(Box::new(ok), Box::new(err))
                }
                _ => Ty::Unknown,
            },
            TypeRefKind::Optional(inner) => {
                Ty::Option(Box::new(self.resolve_type_ref_in(module_id, inner)))
            }
            TypeRefKind::Result { ok, err } => {
                let ok = self.resolve_type_ref_in(module_id, ok);
                let err = err
                    .as_ref()
                    .map(|err| self.resolve_type_ref_in(module_id, err))
                    .unwrap_or(Ty::Unknown);
                Ty::Result(Box::new(ok), Box::new(err))
            }
            TypeRefKind::Refined { base, .. } => {
                let base_ty = self.resolve_simple_type_name(module_id, &base.name);
                Ty::Refined {
                    base: Box::new(base_ty),
                    repr: format!("{}(...)", base.name),
                }
            }
        }
    }

    fn resolve_simple_type_name(&self, module_id: ModuleId, name: &str) -> Ty {
        if name == "Self" {
            return self.current_self_type.clone().unwrap_or(Ty::Unknown);
        }
        if let Some((module_name, item_name)) = split_qualified_type_name(name) {
            let Some(link) = self.modules.get(module_name) else {
                return Ty::Unknown;
            };
            let symbols = self.symbols_in(link.id);
            if symbols.types.contains_key(item_name) {
                return Ty::Struct(item_name.to_string());
            }
            if symbols.enums.contains_key(item_name) {
                return Ty::Enum(item_name.to_string());
            }
            if symbols.configs.contains_key(item_name) {
                return Ty::Config(item_name.to_string());
            }
            return Ty::Unknown;
        }
        if let Some(symbols) = self.symbols_by_id.get(&module_id) {
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
        if let Some(link) = self.import_items.get(name) {
            let symbols = self.symbols_in(link.id);
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
            _ => Ty::Unknown,
        }
    }

    fn lookup_nominal_type_in_scope(&self, name: &str) -> Option<Ty> {
        resolve_nominal_type_in_scope(
            self.module_id,
            name,
            self.modules,
            self.import_items,
            self.symbols_by_id,
        )
    }

    fn lookup_type_field(&self, type_name: &str, field: &str) -> Option<TypeRef> {
        self.global.types.get(type_name).and_then(|info| {
            info.fields
                .iter()
                .find(|candidate| candidate.name == field)
                .map(|field| field.ty.clone())
        })
    }

    fn lookup_config_field(&self, config_name: &str, field: &str) -> Option<TypeRef> {
        self.global.configs.get(config_name).and_then(|info| {
            info.fields
                .iter()
                .find(|candidate| candidate.name == field)
                .map(|field| field.ty.clone())
        })
    }

    fn lookup_enum_variant(&self, enum_name: &str, variant: &str) -> Option<symbols::EnumVariantInfo> {
        self.global.enums.get(enum_name).and_then(|info| {
            info.variants
                .iter()
                .find(|candidate| candidate.name == variant)
                .cloned()
        })
    }

    fn resolve_struct_lit_ty(&self, name: &str) -> Ty {
        if name == "Self" {
            return self.current_self_type.clone().unwrap_or(Ty::Unknown);
        }
        if self.global.types.contains_key(name) {
            Ty::Struct(name.to_string())
        } else if self.global.configs.contains_key(name) {
            Ty::Config(name.to_string())
        } else {
            Ty::Unknown
        }
    }

    fn bind_pattern(&mut self, pattern: &Pattern, ty: &Ty) {
        match &pattern.kind {
            PatternKind::Ident(ident) => {
                self.bind_local(&ident.name, ty.clone());
            }
            PatternKind::EnumVariant { args, .. } => {
                if let Ty::Enum(enum_name) = ty {
                    if let Some(variant) = self.lookup_enum_variant(enum_name, pattern_enum_name(pattern).unwrap_or_default()) {
                        for (arg, payload_ty) in args.iter().zip(variant.payload.iter()) {
                            let payload_ty = self.resolve_type_ref(payload_ty);
                            self.bind_pattern(arg, &payload_ty);
                        }
                    }
                }
            }
            PatternKind::Struct { fields, .. } => {
                if let Ty::Struct(type_name) | Ty::Config(type_name) = ty {
                    for field in fields {
                        let field_ty = self
                            .lookup_type_field(type_name, &field.name.name)
                            .or_else(|| self.lookup_config_field(type_name, &field.name.name))
                            .map(|ty| self.resolve_type_ref(&ty))
                            .unwrap_or(Ty::Unknown);
                        self.bind_pattern(&field.pat, &field_ty);
                    }
                }
            }
            PatternKind::Wildcard | PatternKind::Literal(_) => {}
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

    fn binary_ty(&self, op: &crate::ast::BinaryOp, left: &Ty, right: &Ty) -> Ty {
        match op {
            crate::ast::BinaryOp::Add => match (left, right) {
                (Ty::String, _) | (_, Ty::String) => Ty::String,
                (Ty::Float, _) | (_, Ty::Float) => Ty::Float,
                (Ty::Int, Ty::Int) => Ty::Int,
                _ => Ty::Unknown,
            },
            crate::ast::BinaryOp::Sub
            | crate::ast::BinaryOp::Mul
            | crate::ast::BinaryOp::Div
            | crate::ast::BinaryOp::Mod => match (left, right) {
                (Ty::Float, _) | (_, Ty::Float) => Ty::Float,
                (Ty::Int, Ty::Int) => Ty::Int,
                _ => Ty::Unknown,
            },
            crate::ast::BinaryOp::Eq
            | crate::ast::BinaryOp::NotEq
            | crate::ast::BinaryOp::Lt
            | crate::ast::BinaryOp::LtEq
            | crate::ast::BinaryOp::Gt
            | crate::ast::BinaryOp::GtEq
            | crate::ast::BinaryOp::And
            | crate::ast::BinaryOp::Or => Ty::Bool,
            crate::ast::BinaryOp::Range => Ty::Range(Box::new(left.clone())),
        }
    }

    fn unify_types(&self, left: Ty, right: Ty) -> Ty {
        if left == Ty::Unknown {
            right
        } else if right == Ty::Unknown || left == right {
            left
        } else {
            Ty::Unknown
        }
    }

    fn unbox_transparent(&self, ty: Ty) -> Ty {
        match ty {
            Ty::Refined { base, .. } => self.unbox_transparent(*base),
            Ty::Boxed(inner) => self.unbox_transparent(*inner),
            other => other,
        }
    }

    fn current_self_name(&self) -> Option<String> {
        match &self.current_self_type {
            Some(Ty::Struct(name)) | Some(Ty::Enum(name)) => Some(name.clone()),
            _ => None,
        }
    }

    fn bind_local(&mut self, name: &str, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
    }

    fn lookup_local(&self, name: &str) -> Option<Ty> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
    }

    fn symbols_in(&self, module_id: ModuleId) -> &ModuleSymbols {
        self.symbols_by_id
            .get(&module_id)
            .unwrap_or_else(|| self.local_symbols.expect("missing local symbols"))
    }
}

#[derive(Clone)]
struct MemberResolution {
    ty: Ty,
    synthetic: Option<SyntheticMethod>,
}

fn resolve_nominal_type_in_scope(
    module_id: ModuleId,
    name: &str,
    modules: &ModuleMap,
    import_items: &HashMap<String, ModuleLink>,
    symbols_by_id: &HashMap<ModuleId, ModuleSymbols>,
) -> Option<Ty> {
    if name == "Self" {
        return None;
    }
    if let Some((module_name, item_name)) = split_qualified_type_name(name) {
        let link = modules.get(module_name)?;
        let symbols = symbols_by_id.get(&link.id)?;
        if symbols.types.contains_key(item_name) {
            return Some(Ty::Struct(item_name.to_string()));
        }
        if symbols.enums.contains_key(item_name) {
            return Some(Ty::Enum(item_name.to_string()));
        }
        return None;
    }
    let symbols = symbols_by_id.get(&module_id)?;
    if symbols.types.contains_key(name) {
        return Some(Ty::Struct(name.to_string()));
    }
    if symbols.enums.contains_key(name) {
        return Some(Ty::Enum(name.to_string()));
    }
    let link = import_items.get(name)?;
    let symbols = symbols_by_id.get(&link.id)?;
    if symbols.types.contains_key(name) {
        return Some(Ty::Struct(name.to_string()));
    }
    if symbols.enums.contains_key(name) {
        return Some(Ty::Enum(name.to_string()));
    }
    None
}

fn synth_method_sig(
    module_id: ModuleId,
    method: &FnDecl,
    target_ty: &Ty,
    kind: InterfaceMethodKind,
    unit: &crate::loader::ModuleUnit,
    symbols_by_id: &HashMap<ModuleId, ModuleSymbols>,
) -> FnSig {
    let params = method
        .params
        .iter()
        .map(|param| ParamSig {
            name: param.name.name.clone(),
            ty: resolve_type_ref_with_self(
                module_id,
                &param.ty,
                Some(target_ty),
                &unit.modules,
                &unit.import_items,
                symbols_by_id,
            ),
            has_default: param.default.is_some(),
        })
        .collect::<Vec<_>>();
    let mut all_params = Vec::new();
    if matches!(kind, InterfaceMethodKind::Instance) {
        all_params.push(ParamSig {
            name: "self".to_string(),
            ty: target_ty.clone(),
            has_default: false,
        });
    }
    all_params.extend(params);
    let ret = method
        .ret
        .as_ref()
        .map(|ret| {
            resolve_type_ref_with_self(
                module_id,
                ret,
                Some(target_ty),
                &unit.modules,
                &unit.import_items,
                symbols_by_id,
            )
        })
        .unwrap_or(Ty::Unit);
    FnSig {
        type_params: Vec::new(),
        params: all_params,
        ret: Box::new(ret),
    }
}

fn synthesize_method_decl(method: &FnDecl, synth: &SyntheticMethod) -> FnDecl {
    let mut decl = method.clone();
    decl.name = Ident {
        name: synth.raw_name.clone(),
        span: method.name.span,
    };
    decl.doc = None;
    rewrite_self_in_fn_decl(&mut decl, synth);
    decl
}

fn rewrite_self_in_fn_decl(decl: &mut FnDecl, synth: &SyntheticMethod) {
    let target_ident = Ident {
        name: synth.target.clone(),
        span: decl.span,
    };
    if matches!(synth.kind, InterfaceMethodKind::Instance) {
        decl.params.insert(
            0,
            Param {
                name: Ident {
                    name: "self".to_string(),
                    span: decl.span,
                },
                ty: TypeRef {
                    kind: TypeRefKind::Simple(target_ident.clone()),
                    span: decl.span,
                },
                default: None,
                span: decl.span,
            },
        );
    }
    for param in &mut decl.params {
        rewrite_self_in_type_ref(&mut param.ty, &target_ident);
    }
    if let Some(ret) = &mut decl.ret {
        rewrite_self_in_type_ref(ret, &target_ident);
    }
    rewrite_self_in_block(&mut decl.body, &target_ident);
}

fn rewrite_self_in_type_ref(ty: &mut TypeRef, target: &Ident) {
    match &mut ty.kind {
        TypeRefKind::Simple(ident) if ident.name == "Self" => {
            *ident = target.clone();
        }
        TypeRefKind::Generic { args, .. } => {
            for arg in args {
                rewrite_self_in_type_ref(arg, target);
            }
        }
        TypeRefKind::Optional(inner) => rewrite_self_in_type_ref(inner, target),
        TypeRefKind::Result { ok, err } => {
            rewrite_self_in_type_ref(ok, target);
            if let Some(err) = err {
                rewrite_self_in_type_ref(err, target);
            }
        }
        TypeRefKind::Refined { .. } | TypeRefKind::Simple(_) => {}
    }
}

fn rewrite_self_in_block(block: &mut Block, target: &Ident) {
    for stmt in &mut block.stmts {
        rewrite_self_in_stmt(stmt, target);
    }
}

fn rewrite_self_in_stmt(stmt: &mut Stmt, target: &Ident) {
    match &mut stmt.kind {
        StmtKind::Let { expr, .. } | StmtKind::Var { expr, .. } => rewrite_self_in_expr(expr, target),
        StmtKind::Assign { target: lhs, expr } => {
            rewrite_self_in_expr(lhs, target);
            rewrite_self_in_expr(expr, target);
        }
        StmtKind::Return { expr } => {
            if let Some(expr) = expr {
                rewrite_self_in_expr(expr, target);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            rewrite_self_in_expr(cond, target);
            rewrite_self_in_block(then_block, target);
            for (branch_cond, branch_block) in else_if {
                rewrite_self_in_expr(branch_cond, target);
                rewrite_self_in_block(branch_block, target);
            }
            if let Some(block) = else_block {
                rewrite_self_in_block(block, target);
            }
        }
        StmtKind::Match { expr, cases } => {
            rewrite_self_in_expr(expr, target);
            for (pattern, block) in cases {
                rewrite_self_in_pattern(pattern, target);
                rewrite_self_in_block(block, target);
            }
        }
        StmtKind::For { pat, iter, block } => {
            rewrite_self_in_pattern(pat, target);
            rewrite_self_in_expr(iter, target);
            rewrite_self_in_block(block, target);
        }
        StmtKind::While { cond, block } => {
            rewrite_self_in_expr(cond, target);
            rewrite_self_in_block(block, target);
        }
        StmtKind::Transaction { block } => rewrite_self_in_block(block, target),
        StmtKind::Expr(expr) => rewrite_self_in_expr(expr, target),
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn rewrite_self_in_pattern(pattern: &mut Pattern, target: &Ident) {
    match &mut pattern.kind {
        PatternKind::EnumVariant { args, .. } => {
            for arg in args {
                rewrite_self_in_pattern(arg, target);
            }
        }
        PatternKind::Struct { name, fields } => {
            if name.name == "Self" {
                *name = target.clone();
            }
            for field in fields {
                rewrite_self_in_pattern(&mut field.pat, target);
            }
        }
        PatternKind::Wildcard | PatternKind::Literal(_) | PatternKind::Ident(_) => {}
    }
}

fn rewrite_self_in_expr(expr: &mut Expr, target: &Ident) {
    match &mut expr.kind {
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
        ExprKind::Binary { left, right, .. } | ExprKind::Coalesce { left, right } => {
            rewrite_self_in_expr(left, target);
            rewrite_self_in_expr(right, target);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Await { expr }
        | ExprKind::Box { expr }
        | ExprKind::BangChain { expr, error: None } => rewrite_self_in_expr(expr, target),
        ExprKind::BangChain {
            expr,
            error: Some(error),
        } => {
            rewrite_self_in_expr(expr, target);
            rewrite_self_in_expr(error, target);
        }
        ExprKind::Call {
            callee,
            args,
            type_args: _,
        } => {
            rewrite_self_in_expr(callee, target);
            for arg in args {
                rewrite_self_in_expr(&mut arg.value, target);
            }
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            rewrite_self_in_expr(base, target);
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            rewrite_self_in_expr(base, target);
            rewrite_self_in_expr(index, target);
        }
        ExprKind::StructLit { name, fields } => {
            if name.name == "Self" {
                *name = target.clone();
            }
            for field in fields {
                rewrite_self_in_expr(&mut field.value, target);
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                rewrite_self_in_expr(item, target);
            }
        }
        ExprKind::MapLit(pairs) => {
            for (key, value) in pairs {
                rewrite_self_in_expr(key, target);
                rewrite_self_in_expr(value, target);
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let InterpPart::Expr(expr) = part {
                    rewrite_self_in_expr(expr, target);
                }
            }
        }
        ExprKind::Spawn { block } => rewrite_self_in_block(block, target),
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            rewrite_self_in_expr(cond, target);
            for child in then_children {
                rewrite_self_in_expr(child, target);
            }
            for (branch_cond, branch_children) in else_if {
                rewrite_self_in_expr(branch_cond, target);
                for child in branch_children {
                    rewrite_self_in_expr(child, target);
                }
            }
            for child in else_children {
                rewrite_self_in_expr(child, target);
            }
        }
        ExprKind::HtmlFor {
            pat,
            iter,
            body_children,
        } => {
            rewrite_self_in_pattern(pat, target);
            rewrite_self_in_expr(iter, target);
            for child in body_children {
                rewrite_self_in_expr(child, target);
            }
        }
    }
}

fn resolve_type_ref_with_self(
    module_id: ModuleId,
    ty: &TypeRef,
    current_self: Option<&Ty>,
    modules: &ModuleMap,
    import_items: &HashMap<String, ModuleLink>,
    symbols_by_id: &HashMap<ModuleId, ModuleSymbols>,
) -> Ty {
    match &ty.kind {
        TypeRefKind::Simple(ident) if ident.name == "Self" => {
            current_self.cloned().unwrap_or(Ty::Unknown)
        }
        TypeRefKind::Simple(ident) => resolve_simple_type_name(
            module_id,
            &ident.name,
            modules,
            import_items,
            symbols_by_id,
        ),
        TypeRefKind::Generic { base, args } => match base.name.as_str() {
            "List" => args
                .first()
                .map(|arg| {
                    Ty::List(Box::new(resolve_type_ref_with_self(
                        module_id,
                        arg,
                        current_self,
                        modules,
                        import_items,
                        symbols_by_id,
                    )))
                })
                .unwrap_or(Ty::Unknown),
            "Map" => {
                if args.len() != 2 {
                    return Ty::Unknown;
                }
                let key = resolve_type_ref_with_self(
                    module_id,
                    &args[0],
                    current_self,
                    modules,
                    import_items,
                    symbols_by_id,
                );
                let value = resolve_type_ref_with_self(
                    module_id,
                    &args[1],
                    current_self,
                    modules,
                    import_items,
                    symbols_by_id,
                );
                Ty::Map(Box::new(key), Box::new(value))
            }
            "Option" => args
                .first()
                .map(|arg| {
                    Ty::Option(Box::new(resolve_type_ref_with_self(
                        module_id,
                        arg,
                        current_self,
                        modules,
                        import_items,
                        symbols_by_id,
                    )))
                })
                .unwrap_or(Ty::Unknown),
            "Result" => {
                if args.len() != 2 {
                    return Ty::Unknown;
                }
                let ok = resolve_type_ref_with_self(
                    module_id,
                    &args[0],
                    current_self,
                    modules,
                    import_items,
                    symbols_by_id,
                );
                let err = resolve_type_ref_with_self(
                    module_id,
                    &args[1],
                    current_self,
                    modules,
                    import_items,
                    symbols_by_id,
                );
                Ty::Result(Box::new(ok), Box::new(err))
            }
            _ => Ty::Unknown,
        },
        TypeRefKind::Optional(inner) => Ty::Option(Box::new(resolve_type_ref_with_self(
            module_id,
            inner,
            current_self,
            modules,
            import_items,
            symbols_by_id,
        ))),
        TypeRefKind::Result { ok, err } => {
            let ok = resolve_type_ref_with_self(
                module_id,
                ok,
                current_self,
                modules,
                import_items,
                symbols_by_id,
            );
            let err = err
                .as_ref()
                .map(|err| {
                    resolve_type_ref_with_self(
                        module_id,
                        err,
                        current_self,
                        modules,
                        import_items,
                        symbols_by_id,
                    )
                })
                .unwrap_or(Ty::Unknown);
            Ty::Result(Box::new(ok), Box::new(err))
        }
        TypeRefKind::Refined { base, .. } => Ty::Refined {
            base: Box::new(resolve_simple_type_name(
                module_id,
                &base.name,
                modules,
                import_items,
                symbols_by_id,
            )),
            repr: format!("{}(...)", base.name),
        },
    }
}

fn resolve_simple_type_name(
    module_id: ModuleId,
    name: &str,
    modules: &ModuleMap,
    import_items: &HashMap<String, ModuleLink>,
    symbols_by_id: &HashMap<ModuleId, ModuleSymbols>,
) -> Ty {
    if let Some(ty) = resolve_nominal_type_in_scope(module_id, name, modules, import_items, symbols_by_id) {
        return ty;
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
        _ => Ty::Unknown,
    }
}

fn method_uses_self(
    module_id: ModuleId,
    decl: &ImplDecl,
    method: &FnDecl,
    symbols_by_id: &HashMap<ModuleId, ModuleSymbols>,
) -> bool {
    symbols_by_id
        .get(&module_id)
        .and_then(|symbols| {
            symbols.impls.iter().find(|impl_info| {
                impl_info.interface == decl.interface.name && impl_info.target == decl.target.name
            })
        })
        .and_then(|impl_info| {
            impl_info
                .methods
                .iter()
                .find(|candidate| candidate.name == method.name.name)
        })
        .map(|method| method.uses_self)
        .unwrap_or(false)
}

fn synthetic_method_name(
    interface: &str,
    target: &str,
    method: &str,
    kind: InterfaceMethodKind,
) -> String {
    let kind = match kind {
        InterfaceMethodKind::Instance => "inst",
        InterfaceMethodKind::Associated => "assoc",
    };
    format!(
        "__fuse_impl_{}_{}_{}_{}",
        sanitize_symbol_part(interface),
        sanitize_symbol_part(target),
        sanitize_symbol_part(method),
        kind,
    )
}

fn sanitize_symbol_part(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    out.trim_matches('_').to_string()
}

fn canonical_internal_function_name(module_id: ModuleId, name: &str) -> String {
    format!("m{module_id}::{name}")
}

fn parse_canonical_function_name(name: &str) -> Option<(ModuleId, &str)> {
    let rest = name.strip_prefix('m')?;
    let (module_id, raw_name) = rest.split_once("::")?;
    Some((module_id.parse().ok()?, raw_name))
}

fn split_qualified_type_name(name: &str) -> Option<(&str, &str)> {
    let (module, item) = name.split_once('.')?;
    if module.is_empty() || item.is_empty() {
        return None;
    }
    Some((module, item))
}

fn enum_variant_sig(enum_name: &str, variant: &symbols::EnumVariantInfo) -> FnSig {
    let params = variant
        .payload
        .iter()
        .enumerate()
        .map(|(index, ty)| ParamSig {
            name: format!("arg{}", index + 1),
            ty: resolve_simple_payload_ty(ty),
            has_default: false,
        })
        .collect();
    FnSig {
        type_params: Vec::new(),
        params,
        ret: Box::new(Ty::Enum(enum_name.to_string())),
    }
}

fn resolve_simple_payload_ty(ty: &TypeRef) -> Ty {
    match &ty.kind {
        TypeRefKind::Simple(ident) => match ident.name.as_str() {
            "Int" => Ty::Int,
            "Float" => Ty::Float,
            "Bool" => Ty::Bool,
            "String" => Ty::String,
            other => Ty::Struct(other.to_string()),
        },
        TypeRefKind::Optional(inner) => Ty::Option(Box::new(resolve_simple_payload_ty(inner))),
        TypeRefKind::Generic { base, args } if base.name == "List" && args.len() == 1 => {
            Ty::List(Box::new(resolve_simple_payload_ty(&args[0])))
        }
        _ => Ty::Unknown,
    }
}

fn pattern_enum_name(pattern: &Pattern) -> Option<&str> {
    let PatternKind::EnumVariant { name, .. } = &pattern.kind else {
        return None;
    };
    Some(name.name.as_str())
}
