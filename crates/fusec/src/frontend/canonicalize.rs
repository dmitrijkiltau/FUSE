use std::collections::HashSet;

use crate::ast::{
    Block, CallArg, Expr, ExprKind, Item, Literal, Pattern, PatternKind, Program, Stmt, StmtKind,
    TypeRef, TypeRefKind,
};
use crate::frontend::html_tag_builtin::should_use_html_tag_builtin;
use crate::html_tags;
use crate::loader::ModuleRegistry;

pub fn canonicalize_registry(registry: &mut ModuleRegistry) {
    let mut ids: Vec<_> = registry.modules.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let Some(unit) = registry.modules.get_mut(&id) else {
            continue;
        };
        let mut fn_names = HashSet::new();
        let mut config_names = HashSet::new();
        for item in &unit.program.items {
            match item {
                Item::Fn(decl) => {
                    fn_names.insert(decl.name.name.clone());
                }
                Item::Config(decl) => {
                    config_names.insert(decl.name.name.clone());
                }
                _ => {}
            }
        }
        let import_item_names: HashSet<String> = unit.import_items.keys().cloned().collect();
        let mut canonicalizer = Canonicalizer {
            fn_names,
            config_names,
            import_item_names,
        };
        canonicalizer.canonicalize_program(&mut unit.program.items);
    }
}

pub fn canonicalize_program(program: &mut Program) {
    let mut fn_names = HashSet::new();
    let mut config_names = HashSet::new();
    for item in &program.items {
        match item {
            Item::Fn(decl) => {
                fn_names.insert(decl.name.name.clone());
            }
            Item::Config(decl) => {
                config_names.insert(decl.name.name.clone());
            }
            _ => {}
        }
    }
    let mut canonicalizer = Canonicalizer {
        fn_names,
        config_names,
        import_item_names: HashSet::new(),
    };
    canonicalizer.canonicalize_program(&mut program.items);
}

struct Canonicalizer {
    fn_names: HashSet<String>,
    config_names: HashSet<String>,
    import_item_names: HashSet<String>,
}

impl Canonicalizer {
    fn canonicalize_program(&mut self, items: &mut [Item]) {
        for item in items {
            match item {
                Item::Type(decl) => {
                    for field in &mut decl.fields {
                        self.canonicalize_type_ref(&mut field.ty, &ScopeStack::new());
                        if let Some(default) = &mut field.default {
                            self.canonicalize_expr(default, &mut ScopeStack::new());
                        }
                    }
                }
                Item::Enum(decl) => {
                    for variant in &mut decl.variants {
                        for ty in &mut variant.payload {
                            self.canonicalize_type_ref(ty, &ScopeStack::new());
                        }
                    }
                }
                Item::Fn(decl) => {
                    let mut scope = ScopeStack::new();
                    for param in &mut decl.params {
                        self.canonicalize_type_ref(&mut param.ty, &scope);
                        if let Some(default) = &mut param.default {
                            self.canonicalize_expr(default, &mut scope.clone());
                        }
                        scope.declare(param.name.name.clone());
                    }
                    if let Some(ret) = &mut decl.ret {
                        self.canonicalize_type_ref(ret, &scope);
                    }
                    self.canonicalize_block(&mut decl.body, &mut scope);
                }
                Item::Service(decl) => {
                    for route in &mut decl.routes {
                        if let Some(body_ty) = &mut route.body_type {
                            self.canonicalize_type_ref(body_ty, &ScopeStack::new());
                        }
                        self.canonicalize_type_ref(&mut route.ret_type, &ScopeStack::new());
                        self.canonicalize_block(&mut route.body, &mut ScopeStack::new());
                    }
                }
                Item::Config(decl) => {
                    for field in &mut decl.fields {
                        self.canonicalize_type_ref(&mut field.ty, &ScopeStack::new());
                        self.canonicalize_expr(&mut field.value, &mut ScopeStack::new());
                    }
                }
                Item::App(decl) => self.canonicalize_block(&mut decl.body, &mut ScopeStack::new()),
                Item::Migration(decl) => {
                    self.canonicalize_block(&mut decl.body, &mut ScopeStack::new())
                }
                Item::Test(decl) => self.canonicalize_block(&mut decl.body, &mut ScopeStack::new()),
                Item::Import(_) => {}
            }
        }
    }

    fn canonicalize_type_ref(&mut self, ty: &mut TypeRef, scope: &ScopeStack) {
        match &mut ty.kind {
            TypeRefKind::Simple(_) => {}
            TypeRefKind::Optional(inner) => self.canonicalize_type_ref(inner, scope),
            TypeRefKind::Result { ok, err } => {
                self.canonicalize_type_ref(ok, scope);
                if let Some(err) = err {
                    self.canonicalize_type_ref(err, scope);
                }
            }
            TypeRefKind::Generic { args, .. } => {
                for arg in args {
                    self.canonicalize_type_ref(arg, scope);
                }
            }
            TypeRefKind::Refined { args, .. } => {
                for arg in args {
                    self.canonicalize_expr(arg, &mut scope.clone());
                }
            }
        }
    }

    fn canonicalize_block(&mut self, block: &mut Block, scope: &mut ScopeStack) {
        scope.push();
        for stmt in &mut block.stmts {
            self.canonicalize_stmt(stmt, scope);
        }
        scope.pop();
    }

    fn canonicalize_stmt(&mut self, stmt: &mut Stmt, scope: &mut ScopeStack) {
        match &mut stmt.kind {
            StmtKind::Let { name, ty, expr } | StmtKind::Var { name, ty, expr } => {
                if let Some(ty) = ty {
                    self.canonicalize_type_ref(ty, scope);
                }
                self.canonicalize_expr(expr, scope);
                scope.declare(name.name.clone());
            }
            StmtKind::Assign { target, expr } => {
                self.canonicalize_expr(target, scope);
                self.canonicalize_expr(expr, scope);
            }
            StmtKind::Return { expr } => {
                if let Some(expr) = expr {
                    self.canonicalize_expr(expr, scope);
                }
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                self.canonicalize_expr(cond, scope);
                let mut then_scope = scope.clone();
                self.canonicalize_block(then_block, &mut then_scope);
                for (cond, block) in else_if {
                    self.canonicalize_expr(cond, scope);
                    let mut branch_scope = scope.clone();
                    self.canonicalize_block(block, &mut branch_scope);
                }
                if let Some(block) = else_block {
                    let mut else_scope = scope.clone();
                    self.canonicalize_block(block, &mut else_scope);
                }
            }
            StmtKind::Match { expr, cases } => {
                self.canonicalize_expr(expr, scope);
                for (pattern, block) in cases {
                    let mut case_scope = scope.clone();
                    let mut names = Vec::new();
                    collect_pattern_bindings(pattern, &mut names);
                    for name in names {
                        case_scope.declare(name);
                    }
                    self.canonicalize_block(block, &mut case_scope);
                }
            }
            StmtKind::For { pat, iter, block } => {
                self.canonicalize_expr(iter, scope);
                let mut loop_scope = scope.clone();
                let mut names = Vec::new();
                collect_pattern_bindings(pat, &mut names);
                for name in names {
                    loop_scope.declare(name);
                }
                self.canonicalize_block(block, &mut loop_scope);
            }
            StmtKind::While { cond, block } => {
                self.canonicalize_expr(cond, scope);
                let mut loop_scope = scope.clone();
                self.canonicalize_block(block, &mut loop_scope);
            }
            StmtKind::Expr(expr) => self.canonicalize_expr(expr, scope),
            StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn canonicalize_expr(&mut self, expr: &mut Expr, scope: &mut ScopeStack) {
        match &mut expr.kind {
            ExprKind::Literal(_) | ExprKind::Ident(_) => {}
            ExprKind::Binary { left, right, .. } => {
                self.canonicalize_expr(left, scope);
                self.canonicalize_expr(right, scope);
            }
            ExprKind::Unary { expr, .. } => self.canonicalize_expr(expr, scope),
            ExprKind::Call { callee, args } => {
                self.canonicalize_expr(callee, scope);
                for arg in args.iter_mut() {
                    self.canonicalize_expr(&mut arg.value, scope);
                }
                if let ExprKind::Ident(ident) = &callee.kind {
                    if self.should_use_html_tag_builtin(&ident.name, scope) {
                        canonicalize_html_attr_shorthand(args);
                    }
                }
            }
            ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
                self.canonicalize_expr(base, scope);
            }
            ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
                self.canonicalize_expr(base, scope);
                self.canonicalize_expr(index, scope);
            }
            ExprKind::StructLit { fields, .. } => {
                for field in fields {
                    self.canonicalize_expr(&mut field.value, scope);
                }
            }
            ExprKind::ListLit(items) => {
                for item in items {
                    self.canonicalize_expr(item, scope);
                }
            }
            ExprKind::MapLit(items) => {
                for (key, value) in items {
                    self.canonicalize_expr(key, scope);
                    self.canonicalize_expr(value, scope);
                }
            }
            ExprKind::InterpString(parts) => {
                for part in parts {
                    if let crate::ast::InterpPart::Expr(expr) = part {
                        self.canonicalize_expr(expr, scope);
                    }
                }
            }
            ExprKind::Coalesce { left, right } => {
                self.canonicalize_expr(left, scope);
                self.canonicalize_expr(right, scope);
            }
            ExprKind::BangChain { expr, error } => {
                self.canonicalize_expr(expr, scope);
                if let Some(error) = error {
                    self.canonicalize_expr(error, scope);
                }
            }
            ExprKind::Spawn { block } => {
                let mut spawn_scope = scope.clone();
                self.canonicalize_block(block, &mut spawn_scope);
            }
            ExprKind::Await { expr } | ExprKind::Box { expr } => {
                self.canonicalize_expr(expr, scope);
            }
        }
    }

    fn should_use_html_tag_builtin(&self, name: &str, scope: &ScopeStack) -> bool {
        should_use_html_tag_builtin(
            name,
            scope.contains(name),
            self.fn_names.contains(name),
            self.config_names.contains(name),
            self.import_item_names.contains(name),
        )
    }
}

fn canonicalize_html_attr_shorthand(args: &mut Vec<CallArg>) {
    let has_named = args.iter().any(|arg| arg.name.is_some());
    if !has_named {
        return;
    }

    let mut attrs: Vec<(String, String, crate::span::Span)> = Vec::new();
    let mut child_expr: Option<Expr> = None;
    for arg in args.iter() {
        if let Some(name) = &arg.name {
            let ExprKind::Literal(Literal::String(value)) = &arg.value.kind else {
                return;
            };
            attrs.push((
                html_tags::normalize_attr_name(&name.name),
                value.clone(),
                arg.span,
            ));
            continue;
        }
        if arg.is_block_sugar && child_expr.is_none() {
            child_expr = Some(arg.value.clone());
            continue;
        }
        return;
    }
    if attrs.is_empty() {
        return;
    }

    let map_span = attrs
        .iter()
        .map(|(_, _, span)| *span)
        .reduce(|acc, span| acc.merge(span))
        .unwrap_or_default();
    let map_entries = attrs
        .into_iter()
        .map(|(key, value, span)| {
            let key_expr = Expr {
                kind: ExprKind::Literal(Literal::String(key)),
                span,
            };
            let value_expr = Expr {
                kind: ExprKind::Literal(Literal::String(value)),
                span,
            };
            (key_expr, value_expr)
        })
        .collect();
    let mut canonical = vec![CallArg {
        name: None,
        value: Expr {
            kind: ExprKind::MapLit(map_entries),
            span: map_span,
        },
        span: map_span,
        is_block_sugar: false,
    }];
    if let Some(child) = child_expr {
        let span = child.span;
        canonical.push(CallArg {
            name: None,
            value: child,
            span,
            is_block_sugar: false,
        });
    }
    *args = canonical;
}

fn collect_pattern_bindings(pattern: &Pattern, out: &mut Vec<String>) {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Literal(_) => {}
        PatternKind::Ident(ident) => {
            if ident.name != "_" {
                out.push(ident.name.clone());
            }
        }
        PatternKind::EnumVariant { args, .. } => {
            for arg in args {
                collect_pattern_bindings(arg, out);
            }
        }
        PatternKind::Struct { fields, .. } => {
            for field in fields {
                collect_pattern_bindings(&field.pat, out);
            }
        }
    }
}

#[derive(Clone, Default)]
struct ScopeStack {
    scopes: Vec<HashSet<String>>,
}

impl ScopeStack {
    fn new() -> Self {
        Self {
            scopes: vec![HashSet::new()],
        }
    }

    fn contains(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|scope| scope.contains(name))
    }

    fn declare(&mut self, name: String) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name);
        }
    }

    fn push(&mut self) {
        self.scopes.push(HashSet::new());
    }

    fn pop(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }
}
