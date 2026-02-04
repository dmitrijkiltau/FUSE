use std::collections::{BTreeMap, HashMap, HashSet};

use crate::ast::{
    AppDecl, BinaryOp, Block, EnumDecl, Expr, ExprKind, FnDecl, Ident, Item, Literal, Pattern,
    PatternKind, Program, ServiceDecl, Stmt, StmtKind, TypeDecl, UnaryOp,
};
use crate::loader::{ModuleMap, ModuleRegistry};
use crate::span::Span;

use super::{
    CallKind, Config, ConfigField, Const, EnumInfo, EnumVariantInfo, Function, Instr,
    Program as IrProgram, Service, ServiceRoute, TypeField, TypeInfo,
};

pub fn lower_program(program: &Program, modules: &ModuleMap) -> Result<IrProgram, Vec<String>> {
    let mut lowerer = Lowerer::new(program, modules);
    lowerer.lower();
    if lowerer.errors.is_empty() {
        Ok(IrProgram {
            functions: lowerer.functions,
            apps: lowerer.apps,
            configs: lowerer.configs,
            types: lowerer.types,
            enums: lowerer.enums,
            services: lowerer.services,
        })
    } else {
        Err(lowerer.errors)
    }
}

pub fn lower_registry(registry: &ModuleRegistry) -> Result<IrProgram, Vec<String>> {
    let mut merged = IrProgram {
        functions: HashMap::new(),
        apps: HashMap::new(),
        configs: HashMap::new(),
        types: HashMap::new(),
        enums: HashMap::new(),
        services: HashMap::new(),
    };
    let mut errors = Vec::new();
    for unit in registry.modules.values() {
        match lower_program(&unit.program, &unit.modules) {
            Ok(ir) => {
                merge_named("function", ir.functions, &mut merged.functions, &mut errors);
                merge_named("app", ir.apps, &mut merged.apps, &mut errors);
                merge_named("config", ir.configs, &mut merged.configs, &mut errors);
                merge_named("type", ir.types, &mut merged.types, &mut errors);
                merge_named("enum", ir.enums, &mut merged.enums, &mut errors);
                merge_named("service", ir.services, &mut merged.services, &mut errors);
            }
            Err(mut errs) => errors.append(&mut errs),
        }
    }
    if errors.is_empty() {
        Ok(merged)
    } else {
        Err(errors)
    }
}

struct Lowerer<'a> {
    program: &'a Program,
    modules: &'a ModuleMap,
    functions: HashMap<String, Function>,
    apps: HashMap<String, Function>,
    configs: HashMap<String, Config>,
    types: HashMap<String, TypeInfo>,
    enums: HashMap<String, EnumInfo>,
    services: HashMap<String, Service>,
    errors: Vec<String>,
    config_names: HashSet<String>,
    enum_names: HashSet<String>,
    enum_variant_names: HashSet<String>,
    builtin_names: HashSet<String>,
}

struct LoopContext {
    break_jumps: Vec<usize>,
    continue_jumps: Vec<usize>,
    continue_target: usize,
}

enum AssignStep<'a> {
    Field { name: String, optional: bool },
    Index {
        index: &'a Expr,
        slot: usize,
        optional: bool,
    },
}

impl<'a> Lowerer<'a> {
    fn new(program: &'a Program, modules: &'a ModuleMap) -> Self {
        let mut config_names = HashSet::new();
        let mut enum_names = HashSet::new();
        let mut enum_variant_names = HashSet::new();
        for item in &program.items {
            if let Item::Config(cfg) = item {
                config_names.insert(cfg.name.name.clone());
            } else if let Item::Enum(decl) = item {
                enum_names.insert(decl.name.name.clone());
                for variant in &decl.variants {
                    enum_variant_names.insert(variant.name.name.clone());
                }
            }
        }
        let builtin_names = ["print", "env", "serve", "log", "assert"]
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        Self {
            program,
            modules,
            functions: HashMap::new(),
            apps: HashMap::new(),
            configs: HashMap::new(),
            types: HashMap::new(),
            enums: HashMap::new(),
            services: HashMap::new(),
            errors: Vec::new(),
            config_names,
            enum_names,
            enum_variant_names,
            builtin_names,
        }
    }

    fn lower(&mut self) {
        for item in &self.program.items {
            match item {
                Item::Fn(decl) => self.lower_fn(decl),
                Item::App(app) => self.lower_app(app),
                Item::Config(cfg) => self.lower_config(cfg),
                Item::Type(ty) => self.lower_type(ty),
                Item::Enum(decl) => self.lower_enum(decl),
                Item::Service(service) => self.lower_service(service),
                _ => {}
            }
        }
    }

    fn insert_extra_functions(&mut self, funcs: Vec<Function>) {
        for func in funcs {
            if self.functions.contains_key(&func.name) {
                self.errors
                    .push(format!("duplicate function {}", func.name));
            } else {
                self.functions.insert(func.name.clone(), func);
            }
        }
    }

    fn lower_fn(&mut self, decl: &FnDecl) {
        let mut builder = FuncBuilder::new(
            decl.name.name.clone(),
            decl.ret.clone(),
            &self.config_names,
            &self.enum_names,
            &self.enum_variant_names,
            &self.builtin_names,
            self.modules,
        );
        for param in &decl.params {
            builder.declare_param(&param.name);
        }
        builder.lower_block(&decl.body);
        builder.ensure_return();
        let (func, errors, extra) = builder.finish();
        if let Some(func) = func {
            self.functions.insert(decl.name.name.clone(), func);
        }
        self.errors.extend(errors);
        self.insert_extra_functions(extra);
    }

    fn lower_app(&mut self, app: &AppDecl) {
        let mut builder = FuncBuilder::new(
            format!("app:{}", app.name.value),
            None,
            &self.config_names,
            &self.enum_names,
            &self.enum_variant_names,
            &self.builtin_names,
            self.modules,
        );
        builder.lower_block(&app.body);
        builder.ensure_return();
        let (func, errors, extra) = builder.finish();
        if let Some(func) = func {
            self.apps.insert(app.name.value.clone(), func);
        }
        self.errors.extend(errors);
        self.insert_extra_functions(extra);
    }

    fn lower_config(&mut self, cfg: &crate::ast::ConfigDecl) {
        let mut fields = Vec::new();
        for field in &cfg.fields {
            let fn_name = format!("__config::{}::{}", cfg.name.name, field.name.name);
            let mut builder = FuncBuilder::new(
                fn_name.clone(),
                None,
                &self.config_names,
                &self.enum_names,
                &self.enum_variant_names,
                &self.builtin_names,
                self.modules,
            );
            builder.lower_expr(&field.value);
            builder.emit(Instr::Return);
            let (func, errors, extra) = builder.finish();
            if let Some(func) = func {
                self.functions.insert(fn_name.clone(), func);
            }
            self.errors.extend(errors);
            self.insert_extra_functions(extra);
            fields.push(ConfigField {
                name: field.name.name.clone(),
                ty: field.ty.clone(),
                default_fn: Some(fn_name),
            });
        }
        self.configs.insert(
            cfg.name.name.clone(),
            Config {
                name: cfg.name.name.clone(),
                fields,
            },
        );
    }

    fn lower_type(&mut self, decl: &TypeDecl) {
        let mut fields = Vec::new();
        for field in &decl.fields {
            let default_fn = if let Some(expr) = &field.default {
                let fn_name = format!("__type::{}::{}", decl.name.name, field.name.name);
                let mut builder = FuncBuilder::new(
                    fn_name.clone(),
                    None,
                    &self.config_names,
                    &self.enum_names,
                    &self.enum_variant_names,
                    &self.builtin_names,
                    self.modules,
                );
                builder.lower_expr(expr);
                builder.emit(Instr::Return);
                let (func, errors, extra) = builder.finish();
                if let Some(func) = func {
                    self.functions.insert(fn_name.clone(), func);
                }
                self.errors.extend(errors);
                self.insert_extra_functions(extra);
                Some(fn_name)
            } else {
                None
            };
            fields.push(TypeField {
                name: field.name.name.clone(),
                ty: field.ty.clone(),
                default_fn,
            });
        }
        self.types.insert(
            decl.name.name.clone(),
            TypeInfo {
                name: decl.name.name.clone(),
                fields,
            },
        );
    }

    fn lower_enum(&mut self, decl: &EnumDecl) {
        let variants = decl
            .variants
            .iter()
            .map(|variant| EnumVariantInfo {
                name: variant.name.name.clone(),
                payload: variant.payload.clone(),
            })
            .collect();
        self.enums.insert(
            decl.name.name.clone(),
            EnumInfo {
                name: decl.name.name.clone(),
                variants,
            },
        );
    }

    fn lower_service(&mut self, decl: &ServiceDecl) {
        let mut routes = Vec::new();
        for (idx, route) in decl.routes.iter().enumerate() {
            let handler = format!("__service::{}::{}", decl.name.name, idx);
            let params = extract_route_params(&route.path.value);
            let mut builder = FuncBuilder::new(
                handler.clone(),
                Some(route.ret_type.clone()),
                &self.config_names,
                &self.enum_names,
                &self.enum_variant_names,
                &self.builtin_names,
                self.modules,
            );
            for name in &params {
                let ident = Ident {
                    name: name.clone(),
                    span: Span::default(),
                };
                builder.declare_param(&ident);
            }
            if route.body_type.is_some() {
                let ident = Ident {
                    name: "body".to_string(),
                    span: Span::default(),
                };
            builder.declare_param(&ident);
            }
            builder.lower_block(&route.body);
            builder.ensure_return();
            let (func, errors, extra) = builder.finish();
            if let Some(func) = func {
                self.functions.insert(handler.clone(), func);
            }
            self.errors.extend(errors);
            self.insert_extra_functions(extra);
            routes.push(ServiceRoute {
                verb: route.verb.clone(),
                path: route.path.value.clone(),
                params,
                body_type: route.body_type.clone(),
                ret_type: route.ret_type.clone(),
                handler,
            });
        }
        self.services.insert(
            decl.name.name.clone(),
            Service {
                name: decl.name.name.clone(),
                base_path: decl.base_path.value.clone(),
                routes,
            },
        );
    }
}

fn extract_route_params(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    for segment in split_path(path) {
        if let Some((name, _)) = parse_route_param(&segment) {
            out.push(name);
        }
    }
    out
}

fn split_path(path: &str) -> Vec<String> {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        Vec::new()
    } else {
        trimmed.split('/').map(|s| s.to_string()).collect()
    }
}

fn parse_route_param(segment: &str) -> Option<(String, String)> {
    if !segment.starts_with('{') || !segment.ends_with('}') {
        return None;
    }
    let inner = &segment[1..segment.len() - 1];
    let mut parts = inner.splitn(2, ':');
    let name = parts.next().unwrap_or("").trim();
    let ty = parts.next().unwrap_or("").trim();
    if name.is_empty() || ty.is_empty() {
        return None;
    }
    Some((name.to_string(), ty.to_string()))
}

fn merge_named<T>(
    kind: &str,
    items: HashMap<String, T>,
    out: &mut HashMap<String, T>,
    errors: &mut Vec<String>,
) {
    for (name, value) in items {
        if out.contains_key(&name) {
            errors.push(format!("duplicate {kind} {name}"));
        } else {
            out.insert(name, value);
        }
    }
}

struct FuncBuilder {
    name: String,
    params: Vec<String>,
    ret: Option<crate::ast::TypeRef>,
    code: Vec<Instr>,
    locals: usize,
    scopes: Vec<HashMap<String, usize>>,
    errors: Vec<String>,
    extra_functions: Vec<Function>,
    spawn_counter: usize,
    config_names: HashSet<String>,
    enum_names: HashSet<String>,
    enum_variant_names: HashSet<String>,
    builtin_names: HashSet<String>,
    modules: ModuleMap,
    loop_stack: Vec<LoopContext>,
}

impl FuncBuilder {
    fn new(
        name: String,
        ret: Option<crate::ast::TypeRef>,
        config_names: &HashSet<String>,
        enum_names: &HashSet<String>,
        enum_variant_names: &HashSet<String>,
        builtin_names: &HashSet<String>,
        modules: &ModuleMap,
    ) -> Self {
        Self {
            name,
            params: Vec::new(),
            ret,
            code: Vec::new(),
            locals: 0,
            scopes: vec![HashMap::new()],
            errors: Vec::new(),
            extra_functions: Vec::new(),
            spawn_counter: 0,
            config_names: config_names.clone(),
            enum_names: enum_names.clone(),
            enum_variant_names: enum_variant_names.clone(),
            builtin_names: builtin_names.clone(),
            modules: modules.clone(),
            loop_stack: Vec::new(),
        }
    }

    fn finish(self) -> (Option<Function>, Vec<String>, Vec<Function>) {
        let func = if self.errors.is_empty() {
            Some(Function {
                name: self.name,
                params: self.params,
                ret: self.ret,
                locals: self.locals,
                code: self.code,
            })
        } else {
            None
        };
        (func, self.errors, self.extra_functions)
    }

    fn emit(&mut self, instr: Instr) {
        self.code.push(instr);
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
        if self.scopes.is_empty() {
            self.scopes.push(HashMap::new());
        }
    }

    fn declare(&mut self, ident: &Ident) -> usize {
        let slot = self.locals;
        self.locals += 1;
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(ident.name.clone(), slot);
        }
        slot
    }

    fn declare_param(&mut self, ident: &Ident) -> usize {
        let slot = self.declare(ident);
        self.params.push(ident.name.clone());
        slot
    }

    fn declare_temp(&mut self) -> usize {
        let slot = self.locals;
        self.locals += 1;
        slot
    }

    fn next_spawn_name(&mut self) -> String {
        let id = self.spawn_counter;
        self.spawn_counter += 1;
        format!("__spawn::{}::{}", self.name, id)
    }

    fn capture_locals(&self) -> Vec<(String, usize)> {
        let mut captured: BTreeMap<String, usize> = BTreeMap::new();
        for scope in self.scopes.iter().rev() {
            for (name, slot) in scope {
                captured.entry(name.clone()).or_insert(*slot);
            }
        }
        captured.into_iter().collect()
    }

    fn collect_bindings(&self, pat: &Pattern, out: &mut Vec<Ident>) {
        match &pat.kind {
            PatternKind::Ident(ident) => {
                if !matches!(ident.name.as_str(), "Some" | "None" | "Ok" | "Err")
                    && !self.enum_variant_names.contains(&ident.name)
                {
                    out.push(ident.clone());
                }
            }
            PatternKind::EnumVariant { args, .. } => {
                for arg in args {
                    self.collect_bindings(arg, out);
                }
            }
            PatternKind::Struct { fields, .. } => {
                for field in fields {
                    self.collect_bindings(&field.pat, out);
                }
            }
            PatternKind::Wildcard | PatternKind::Literal(_) => {}
        }
    }

    fn collect_assign_steps<'e>(
        &mut self,
        target: &'e Expr,
        out: &mut Vec<AssignStep<'e>>,
    ) -> Option<String> {
        match &target.kind {
            ExprKind::Ident(ident) => Some(ident.name.clone()),
            ExprKind::Member { base, name } => {
                let root = self.collect_assign_steps(base, out)?;
                out.push(AssignStep::Field {
                    name: name.name.clone(),
                    optional: false,
                });
                Some(root)
            }
            ExprKind::OptionalMember { base, name } => {
                let root = self.collect_assign_steps(base, out)?;
                out.push(AssignStep::Field {
                    name: name.name.clone(),
                    optional: true,
                });
                Some(root)
            }
            ExprKind::Index { base, index } => {
                let root = self.collect_assign_steps(base, out)?;
                let slot = self.declare_temp();
                out.push(AssignStep::Index {
                    index,
                    slot,
                    optional: false,
                });
                Some(root)
            }
            ExprKind::OptionalIndex { base, index } => {
                let root = self.collect_assign_steps(base, out)?;
                let slot = self.declare_temp();
                out.push(AssignStep::Index {
                    index,
                    slot,
                    optional: true,
                });
                Some(root)
            }
            _ => None,
        }
    }

    fn resolve(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some(slot) = scope.get(name) {
                return Some(*slot);
            }
        }
        None
    }

    fn ensure_return(&mut self) {
        if !matches!(self.code.last(), Some(Instr::Return)) {
            self.emit(Instr::Push(Const::Unit));
            self.emit(Instr::Return);
        }
    }

    fn lower_block(&mut self, block: &Block) {
        self.enter_scope();
        if block.stmts.is_empty() {
            self.emit(Instr::Push(Const::Unit));
            self.exit_scope();
            return;
        }
        for (idx, stmt) in block.stmts.iter().enumerate() {
            let is_last = idx + 1 == block.stmts.len();
            self.lower_stmt(stmt);
            if !is_last {
                self.emit(Instr::Pop);
            }
        }
        self.exit_scope();
    }

    fn lower_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Let { name, expr, .. } | StmtKind::Var { name, expr, .. } => {
                let slot = self.declare(name);
                self.lower_expr(expr);
                self.emit(Instr::StoreLocal(slot));
                self.emit(Instr::Push(Const::Unit));
            }
            StmtKind::Assign { target, expr } => match &target.kind {
                ExprKind::Ident(ident) => match self.resolve(&ident.name) {
                    Some(slot) => {
                        self.lower_expr(expr);
                        self.emit(Instr::StoreLocal(slot));
                        self.emit(Instr::Push(Const::Unit));
                    }
                    None => self.errors.push(format!("unknown variable {}", ident.name)),
                },
                ExprKind::Member { .. }
                | ExprKind::OptionalMember { .. }
                | ExprKind::Index { .. }
                | ExprKind::OptionalIndex { .. } => {
                    let mut steps = Vec::new();
                    let Some(root) = self.collect_assign_steps(target, &mut steps) else {
                        self.errors
                            .push("unsupported assignment target".to_string());
                        self.emit(Instr::Push(Const::Unit));
                        return;
                    };
                    if steps.is_empty() {
                        self.errors
                            .push("unsupported assignment target".to_string());
                        self.emit(Instr::Push(Const::Unit));
                        return;
                    }
                    let Some(slot) = self.resolve(&root) else {
                        self.errors.push(format!("unknown variable {}", root));
                        self.emit(Instr::Push(Const::Unit));
                        return;
                    };
                    self.emit(Instr::LoadLocal(slot));
                    for (idx, step) in steps.iter().enumerate() {
                        let is_last = idx + 1 == steps.len();
                        match step {
                            AssignStep::Field { name, optional } => {
                                if *optional {
                                    self.emit_optional_assign_guard();
                                }
                                if !is_last {
                                    self.emit(Instr::Dup);
                                    self.emit(Instr::GetField { field: name.clone() });
                                }
                            }
                            AssignStep::Index {
                                index,
                                slot: index_slot,
                                optional,
                            } => {
                                if *optional {
                                    self.emit_optional_assign_guard();
                                }
                                if !is_last {
                                    self.emit(Instr::Dup);
                                    self.lower_expr(index);
                                    self.emit(Instr::StoreLocal(*index_slot));
                                    self.emit(Instr::LoadLocal(*index_slot));
                                    self.emit(Instr::GetIndex);
                                } else {
                                    self.lower_expr(index);
                                    self.emit(Instr::StoreLocal(*index_slot));
                                }
                            }
                        }
                    }
                    let last = steps.last().unwrap();
                    match last {
                        AssignStep::Field { name, .. } => {
                            self.lower_expr(expr);
                            self.emit(Instr::SetField { field: name.clone() });
                        }
                        AssignStep::Index { slot: index_slot, .. } => {
                            self.emit(Instr::LoadLocal(*index_slot));
                            self.lower_expr(expr);
                            self.emit(Instr::SetIndex);
                        }
                    }
                    if steps.len() > 1 {
                        for step in steps[..steps.len() - 1].iter().rev() {
                            match step {
                                AssignStep::Field { name, .. } => {
                                    self.emit(Instr::SetField { field: name.clone() });
                                }
                                AssignStep::Index { slot: index_slot, .. } => {
                                    self.emit(Instr::LoadLocal(*index_slot));
                                    self.emit(Instr::SetIndex);
                                }
                            }
                        }
                    }
                    self.emit(Instr::StoreLocal(slot));
                    self.emit(Instr::Push(Const::Unit));
                }
                _ => self.errors.push("unsupported assignment target".to_string()),
            },
            StmtKind::Return { expr } => {
                if let Some(expr) = expr {
                    self.lower_expr(expr);
                } else {
                    self.emit(Instr::Push(Const::Unit));
                }
                self.emit(Instr::Return);
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                self.lower_if(cond, then_block, else_if, else_block.as_ref());
            }
            StmtKind::Expr(expr) => {
                self.lower_expr(expr);
            }
            StmtKind::Match { expr, cases } => {
                self.lower_match(expr, cases);
            }
            StmtKind::While { cond, block } => {
                self.lower_while(cond, block);
            }
            StmtKind::For { pat, iter, block } => {
                self.lower_for(pat, iter, block);
            }
            StmtKind::Break => {
                self.lower_break();
            }
            StmtKind::Continue => {
                self.lower_continue();
            }
        }
    }

    fn lower_match(&mut self, expr: &Expr, cases: &[(Pattern, Block)]) {
        let temp = self.declare_temp();
        self.lower_expr(expr);
        self.emit(Instr::StoreLocal(temp));

        let mut end_jumps = Vec::new();
        for (pat, block) in cases {
            self.enter_scope();
            let mut bindings = Vec::new();
            self.collect_bindings(pat, &mut bindings);
            let mut binding_slots = Vec::new();
            let mut seen = HashSet::new();
            for ident in bindings {
                if seen.insert(ident.name.clone()) {
                    let slot = self.declare(&ident);
                    binding_slots.push((ident.name.clone(), slot));
                }
            }
            let match_idx = self.emit_match(temp, pat.clone(), binding_slots);
            self.lower_block(block);
            self.exit_scope();
            end_jumps.push(self.emit_placeholder());
            self.emit(Instr::Jump(0));
            let next_case = self.code.len();
            self.patch_match_jump(match_idx, next_case);
        }

        self.emit(Instr::Push(Const::Unit));
        let end = self.code.len();
        for jump in end_jumps {
            self.patch_jump(jump, end);
        }
    }

    fn lower_if(
        &mut self,
        cond: &Expr,
        then_block: &Block,
        else_if: &[(Expr, Block)],
        else_block: Option<&Block>,
    ) {
        let mut end_jumps = Vec::new();

        self.lower_expr(cond);
        let jump_to_else = self.emit_placeholder();
        self.emit(Instr::JumpIfFalse(0));
        self.lower_block(then_block);
        end_jumps.push(self.emit_placeholder());
        self.emit(Instr::Jump(0));
        self.patch_jump(jump_to_else, self.code.len());

        let pending_else = else_block;
        for (cond, block) in else_if {
            self.lower_expr(cond);
            let jump_next = self.emit_placeholder();
            self.emit(Instr::JumpIfFalse(0));
            self.lower_block(block);
            end_jumps.push(self.emit_placeholder());
            self.emit(Instr::Jump(0));
            self.patch_jump(jump_next, self.code.len());
        }

        if let Some(block) = pending_else {
            self.lower_block(block);
        } else {
            self.emit(Instr::Push(Const::Unit));
        }

        let end = self.code.len();
        for jump in end_jumps {
            self.patch_jump(jump, end);
        }
    }

    fn lower_while(&mut self, cond: &Expr, block: &Block) {
        let loop_start = self.code.len();
        self.lower_expr(cond);
        let jump_end = self.emit_placeholder();
        self.emit(Instr::JumpIfFalse(0));

        self.loop_stack.push(LoopContext {
            break_jumps: Vec::new(),
            continue_jumps: Vec::new(),
            continue_target: loop_start,
        });

        self.lower_block(block);
        self.emit(Instr::Pop);
        self.emit(Instr::Jump(loop_start));

        let loop_ctx = self.loop_stack.pop().unwrap_or(LoopContext {
            break_jumps: Vec::new(),
            continue_jumps: Vec::new(),
            continue_target: loop_start,
        });

        let end = self.code.len();
        self.patch_jump(jump_end, end);
        for jump in loop_ctx.break_jumps {
            self.patch_jump(jump, end);
        }
        for jump in loop_ctx.continue_jumps {
            self.patch_jump(jump, loop_ctx.continue_target);
        }

        self.emit(Instr::Push(Const::Unit));
    }

    fn lower_for(&mut self, pat: &Pattern, iter: &Expr, block: &Block) {
        self.lower_expr(iter);
        self.emit(Instr::IterInit);
        let iter_slot = self.declare_temp();
        self.emit(Instr::StoreLocal(iter_slot));
        let item_slot = self.declare_temp();

        self.enter_scope();
        let mut bindings = Vec::new();
        self.collect_bindings(pat, &mut bindings);
        let mut binding_slots = Vec::new();
        let mut seen = HashSet::new();
        for ident in bindings {
            if seen.insert(ident.name.clone()) {
                let slot = self.declare(&ident);
                binding_slots.push((ident.name.clone(), slot));
            }
        }

        let loop_start = self.code.len();
        self.emit(Instr::LoadLocal(iter_slot));
        let iter_next = self.emit_placeholder();
        self.emit(Instr::IterNext { jump: 0 });
        self.emit(Instr::StoreLocal(iter_slot));
        self.emit(Instr::StoreLocal(item_slot));

        self.loop_stack.push(LoopContext {
            break_jumps: Vec::new(),
            continue_jumps: Vec::new(),
            continue_target: loop_start,
        });

        let match_idx = self.emit_match(item_slot, pat.clone(), binding_slots);
        self.lower_block(block);
        self.emit(Instr::Pop);
        self.emit(Instr::Jump(loop_start));

        let loop_ctx = self.loop_stack.pop().unwrap_or(LoopContext {
            break_jumps: Vec::new(),
            continue_jumps: Vec::new(),
            continue_target: loop_start,
        });

        let pattern_error = self.code.len();
        self.emit(Instr::RuntimeError(
            "for pattern did not match value".to_string(),
        ));

        let end = self.code.len();
        self.patch_jump(iter_next, end);
        self.patch_match_jump(match_idx, pattern_error);
        for jump in loop_ctx.break_jumps {
            self.patch_jump(jump, end);
        }
        for jump in loop_ctx.continue_jumps {
            self.patch_jump(jump, loop_ctx.continue_target);
        }

        self.exit_scope();
        self.emit(Instr::Push(Const::Unit));
    }

    fn lower_break(&mut self) {
        if self.loop_stack.is_empty() {
            self.errors.push("break used outside of loop".to_string());
            self.emit(Instr::Push(Const::Unit));
            return;
        }
        let jump = self.emit_placeholder();
        self.emit(Instr::Jump(0));
        if let Some(loop_ctx) = self.loop_stack.last_mut() {
            loop_ctx.break_jumps.push(jump);
        }
    }

    fn lower_continue(&mut self) {
        if self.loop_stack.is_empty() {
            self.errors.push("continue used outside of loop".to_string());
            self.emit(Instr::Push(Const::Unit));
            return;
        }
        let jump = self.emit_placeholder();
        self.emit(Instr::Jump(0));
        if let Some(loop_ctx) = self.loop_stack.last_mut() {
            loop_ctx.continue_jumps.push(jump);
        }
    }

    fn emit_placeholder(&mut self) -> usize {
        self.code.len()
    }

    fn emit_optional_assign_guard(&mut self) {
        self.emit(Instr::Dup);
        let jump_err = self.emit_placeholder();
        self.emit(Instr::JumpIfNull(0));
        let jump_ok = self.emit_placeholder();
        self.emit(Instr::Jump(0));
        let err_pos = self.code.len();
        self.emit(Instr::RuntimeError(
            "cannot assign through optional access".to_string(),
        ));
        let ok_pos = self.code.len();
        self.patch_jump(jump_err, err_pos);
        self.patch_jump(jump_ok, ok_pos);
    }

    fn patch_jump(&mut self, at: usize, target: usize) {
        match self.code.get_mut(at) {
            Some(Instr::Jump(slot)) => *slot = target,
            Some(Instr::JumpIfFalse(slot)) => *slot = target,
            Some(Instr::JumpIfNull(slot)) => *slot = target,
            Some(Instr::IterNext { jump }) => *jump = target,
            _ => {
                self.errors.push("invalid jump patch".to_string());
            }
        }
    }

    fn emit_match(&mut self, slot: usize, pat: Pattern, bindings: Vec<(String, usize)>) -> usize {
        let idx = self.code.len();
        self.code.push(Instr::MatchLocal {
            slot,
            pat,
            bindings,
            jump: 0,
        });
        idx
    }

    fn patch_match_jump(&mut self, at: usize, target: usize) {
        match self.code.get_mut(at) {
            Some(Instr::MatchLocal { jump, .. }) => *jump = target,
            _ => {
                self.errors.push("invalid match patch".to_string());
            }
        }
    }

    fn lower_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Literal(lit) => self.emit(Instr::Push(self.const_from_lit(lit))),
            ExprKind::Ident(ident) => {
                if let Some(slot) = self.resolve(&ident.name) {
                    self.emit(Instr::LoadLocal(slot));
                } else {
                    self.errors
                        .push(format!("unknown identifier {}", ident.name));
                }
            }
            ExprKind::Binary { op, left, right } => {
                self.lower_expr(left);
                self.lower_expr(right);
                match op {
                    BinaryOp::Add => self.emit(Instr::Add),
                    BinaryOp::Sub => self.emit(Instr::Sub),
                    BinaryOp::Mul => self.emit(Instr::Mul),
                    BinaryOp::Div => self.emit(Instr::Div),
                    BinaryOp::Mod => self.emit(Instr::Mod),
                    BinaryOp::Eq => self.emit(Instr::Eq),
                    BinaryOp::NotEq => self.emit(Instr::NotEq),
                    BinaryOp::Lt => self.emit(Instr::Lt),
                    BinaryOp::LtEq => self.emit(Instr::LtEq),
                    BinaryOp::Gt => self.emit(Instr::Gt),
                    BinaryOp::GtEq => self.emit(Instr::GtEq),
                    BinaryOp::And => self.emit(Instr::And),
                    BinaryOp::Or => self.emit(Instr::Or),
                    BinaryOp::Range => {
                        self.emit(Instr::Call {
                            name: "range".to_string(),
                            argc: 2,
                            kind: CallKind::Builtin,
                        });
                    }
                }
            }
            ExprKind::Unary { op, expr } => {
                self.lower_expr(expr);
                match op {
                    UnaryOp::Neg => self.emit(Instr::Neg),
                    UnaryOp::Not => self.emit(Instr::Not),
                }
            }
            ExprKind::Call { callee, args } => {
                match &callee.kind {
                    ExprKind::Ident(ident) => {
                        for arg in args {
                            self.lower_expr(&arg.value);
                        }
                        let kind = if self.builtin_names.contains(&ident.name) {
                            CallKind::Builtin
                        } else {
                            CallKind::Function
                        };
                        self.emit(Instr::Call {
                            name: ident.name.clone(),
                            argc: args.len(),
                            kind,
                        });
                    }
                    ExprKind::Member { base, name } => {
                        if let ExprKind::Ident(ident) = &base.kind {
                            if ident.name == "db" || ident.name == "task" {
                                for arg in args {
                                    self.lower_expr(&arg.value);
                                }
                                self.emit(Instr::Call {
                                    name: format!("{}.{}", ident.name, name.name),
                                    argc: args.len(),
                                    kind: CallKind::Builtin,
                                });
                                return;
                            }
                        }
                        if let ExprKind::Ident(module_ident) = &base.kind {
                            if let Some(module) = self.modules.get(&module_ident.name) {
                                if module.exports.functions.contains(&name.name) {
                                    for arg in args {
                                        self.lower_expr(&arg.value);
                                    }
                                    self.emit(Instr::Call {
                                        name: name.name.clone(),
                                        argc: args.len(),
                                        kind: CallKind::Function,
                                    });
                                    return;
                                }
                            }
                        }
                        if let ExprKind::Member {
                            base: inner_base,
                            name: inner_name,
                        } = &base.kind
                        {
                            if let ExprKind::Ident(module_ident) = &inner_base.kind {
                                if let Some(module) = self.modules.get(&module_ident.name) {
                                    if module.exports.enums.contains(&inner_name.name) {
                                        for arg in args {
                                            self.lower_expr(&arg.value);
                                        }
                                        self.emit(Instr::MakeEnum {
                                            name: inner_name.name.clone(),
                                            variant: name.name.clone(),
                                            argc: args.len(),
                                        });
                                        return;
                                    }
                                }
                            }
                        }
                        if let ExprKind::Ident(enum_name) = &base.kind {
                            if self.enum_names.contains(&enum_name.name) {
                                for arg in args {
                                    self.lower_expr(&arg.value);
                                }
                                self.emit(Instr::MakeEnum {
                                    name: enum_name.name.clone(),
                                    variant: name.name.clone(),
                                    argc: args.len(),
                                });
                            } else {
                                self.errors
                                    .push("call target not supported in VM yet".to_string());
                            }
                        } else {
                            self.errors
                                .push("call target not supported in VM yet".to_string());
                        }
                    }
                    _ => {
                        self.errors
                            .push("call target not supported in VM yet".to_string());
                    }
                }
            }
            ExprKind::Member { base, name } => {
                if let ExprKind::Member {
                    base: inner_base,
                    name: inner_name,
                } = &base.kind
                {
                    if let ExprKind::Ident(module_ident) = &inner_base.kind {
                        if let Some(module) = self.modules.get(&module_ident.name) {
                            if module.exports.configs.contains(&inner_name.name) {
                                self.emit(Instr::LoadConfigField {
                                    config: inner_name.name.clone(),
                                    field: name.name.clone(),
                                });
                                return;
                            }
                            if module.exports.enums.contains(&inner_name.name) {
                                self.emit(Instr::MakeEnum {
                                    name: inner_name.name.clone(),
                                    variant: name.name.clone(),
                                    argc: 0,
                                });
                                return;
                            }
                        }
                    }
                }
                if let ExprKind::Ident(module_ident) = &base.kind {
                    if self.modules.contains(&module_ident.name) {
                        self.errors
                            .push("module members are not values in the VM yet".to_string());
                        return;
                    }
                }
                if let ExprKind::Ident(ident) = &base.kind {
                    if self.config_names.contains(&ident.name) {
                        self.emit(Instr::LoadConfigField {
                            config: ident.name.clone(),
                            field: name.name.clone(),
                        });
                    } else if self.enum_names.contains(&ident.name) {
                        self.emit(Instr::MakeEnum {
                            name: ident.name.clone(),
                            variant: name.name.clone(),
                            argc: 0,
                        });
                    } else {
                        self.lower_expr(base);
                        self.emit(Instr::GetField {
                            field: name.name.clone(),
                        });
                    }
                } else {
                    self.lower_expr(base);
                    self.emit(Instr::GetField {
                        field: name.name.clone(),
                    });
                }
            }
            ExprKind::OptionalMember { base, name } => {
                self.lower_expr(base);
                self.emit(Instr::Dup);
                let jump_idx = self.emit_placeholder();
                self.emit(Instr::JumpIfNull(0));
                self.emit(Instr::GetField {
                    field: name.name.clone(),
                });
                self.patch_jump(jump_idx, self.code.len());
            }
            ExprKind::Index { base, index } => {
                self.lower_expr(base);
                self.lower_expr(index);
                self.emit(Instr::GetIndex);
            }
            ExprKind::OptionalIndex { base, index } => {
                self.lower_expr(base);
                self.emit(Instr::Dup);
                let jump_idx = self.emit_placeholder();
                self.emit(Instr::JumpIfNull(0));
                self.lower_expr(index);
                self.emit(Instr::GetIndex);
                self.patch_jump(jump_idx, self.code.len());
            }
            ExprKind::StructLit { name, fields } => {
                let mut field_names = Vec::new();
                for field in fields {
                    self.lower_expr(&field.value);
                    field_names.push(field.name.name.clone());
                }
                self.emit(Instr::MakeStruct {
                    name: name.name.clone(),
                    fields: field_names,
                });
            }
            ExprKind::ListLit(items) => {
                for item in items {
                    self.lower_expr(item);
                }
                self.emit(Instr::MakeList { len: items.len() });
            }
            ExprKind::MapLit(pairs) => {
                for (key, value) in pairs {
                    self.lower_expr(key);
                    self.lower_expr(value);
                }
                self.emit(Instr::MakeMap { len: pairs.len() });
            }
            ExprKind::InterpString(parts) => {
                for part in parts {
                    match part {
                        crate::ast::InterpPart::Text(text) => {
                            self.emit(Instr::Push(Const::String(text.clone())));
                        }
                        crate::ast::InterpPart::Expr(expr) => {
                            self.lower_expr(expr);
                        }
                    }
                }
                self.emit(Instr::InterpString { parts: parts.len() });
            }
            ExprKind::Coalesce { left, right } => {
                self.lower_expr(left);
                self.emit(Instr::Dup);
                let jump_else = self.emit_placeholder();
                self.emit(Instr::JumpIfNull(0));
                let jump_end = self.emit_placeholder();
                self.emit(Instr::Jump(0));
                self.patch_jump(jump_else, self.code.len());
                self.emit(Instr::Pop);
                self.lower_expr(right);
                self.patch_jump(jump_end, self.code.len());
            }
            ExprKind::BangChain { expr, error } => {
                self.lower_expr(expr);
                if let Some(error) = error {
                    self.lower_expr(error);
                    self.emit(Instr::Bang { has_error: true });
                } else {
                    self.emit(Instr::Bang { has_error: false });
                }
            }
            ExprKind::Box { expr } => {
                self.lower_expr(expr);
                self.emit(Instr::MakeBox);
            }
            ExprKind::Spawn { block } => {
                let captured = self.capture_locals();
                let spawn_name = self.next_spawn_name();
                let mut builder = FuncBuilder::new(
                    spawn_name.clone(),
                    None,
                    &self.config_names,
                    &self.enum_names,
                    &self.enum_variant_names,
                    &self.builtin_names,
                    &self.modules,
                );
                for (name, _) in &captured {
                    let ident = Ident {
                        name: name.clone(),
                        span: Span::default(),
                    };
                    builder.declare_param(&ident);
                }
                builder.lower_block(block);
                builder.emit(Instr::Return);
                builder.ensure_return();
                let (func, errors, extra) = builder.finish();
                if let Some(func) = func {
                    self.extra_functions.push(func);
                }
                self.errors.extend(errors);
                self.extra_functions.extend(extra);
                for (_, slot) in &captured {
                    self.emit(Instr::LoadLocal(*slot));
                }
                self.emit(Instr::Spawn {
                    name: spawn_name,
                    argc: captured.len(),
                });
            }
            ExprKind::Await { expr } => {
                self.lower_expr(expr);
                self.emit(Instr::Await);
            }
        }
    }

    fn const_from_lit(&self, lit: &Literal) -> Const {
        match lit {
            Literal::Int(v) => Const::Int(*v),
            Literal::Float(v) => Const::Float(*v),
            Literal::Bool(v) => Const::Bool(*v),
            Literal::String(v) => Const::String(v.clone()),
            Literal::Null => Const::Null,
        }
    }
}
