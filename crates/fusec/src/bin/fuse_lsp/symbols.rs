use std::collections::{HashMap, HashSet};

use fusec::ast::{
    Block, ConfigDecl, Doc, EnumDecl, Expr, ExprKind, FnDecl, Ident, ImportDecl, ImportSpec, Item,
    Pattern, PatternKind, Program, ServiceDecl, Stmt, StmtKind, TypeDecl, TypeDerive, TypeRef,
    TypeRefKind,
};
use fusec::span::Span;

pub(crate) struct Index {
    pub(crate) defs: Vec<SymbolDef>,
    pub(crate) refs: Vec<SymbolRef>,
    pub(crate) calls: Vec<CallRef>,
    pub(crate) qualified_calls: Vec<QualifiedCallRef>,
}

impl Index {
    pub(crate) fn definition_at(&self, offset: usize) -> Option<usize> {
        if let Some(def_id) = self.reference_at(offset) {
            return Some(def_id);
        }
        self.def_at(offset)
    }

    fn reference_at(&self, offset: usize) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for reference in &self.refs {
            if span_contains(reference.span, offset) {
                let size = reference.span.end.saturating_sub(reference.span.start);
                if best.map_or(true, |(_, best_size)| size < best_size) {
                    best = Some((reference.target, size));
                }
            }
        }
        best.map(|(def_id, _)| def_id)
    }

    fn def_at(&self, offset: usize) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for (id, def) in self.defs.iter().enumerate() {
            if span_contains(def.span, offset) {
                let size = def.span.end.saturating_sub(def.span.start);
                if best.map_or(true, |(_, best_size)| size < best_size) {
                    best = Some((id, size));
                }
            }
        }
        best.map(|(id, _)| id)
    }
}

#[derive(Clone)]
pub(crate) struct SymbolDef {
    pub(crate) name: String,
    pub(crate) span: Span,
    pub(crate) kind: SymbolKind,
    pub(crate) detail: String,
    pub(crate) doc: Option<String>,
    pub(crate) container: Option<String>,
}

pub(crate) struct SymbolRef {
    pub(crate) span: Span,
    pub(crate) target: usize,
}

pub(crate) struct CallRef {
    pub(crate) caller: usize,
    pub(crate) callee: usize,
    pub(crate) span: Span,
}

pub(crate) struct QualifiedCallRef {
    pub(crate) caller: usize,
    pub(crate) module: String,
    pub(crate) item: String,
    pub(crate) span: Span,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SymbolKind {
    Module,
    Type,
    Enum,
    EnumVariant,
    Function,
    Config,
    Service,
    App,
    Migration,
    Test,
    Param,
    Variable,
    Field,
}

impl SymbolKind {
    pub(crate) fn lsp_kind(self) -> u32 {
        match self {
            SymbolKind::Module => 2,
            SymbolKind::Type => 23,
            SymbolKind::Enum => 10,
            SymbolKind::EnumVariant => 22,
            SymbolKind::Function => 12,
            SymbolKind::Config => 23,
            SymbolKind::Service => 11,
            SymbolKind::App => 5,
            SymbolKind::Migration => 12,
            SymbolKind::Test => 12,
            SymbolKind::Param => 13,
            SymbolKind::Variable => 13,
            SymbolKind::Field => 8,
        }
    }

    pub(crate) fn to_u8(self) -> u8 {
        match self {
            SymbolKind::Module => 0,
            SymbolKind::Type => 1,
            SymbolKind::Enum => 2,
            SymbolKind::EnumVariant => 3,
            SymbolKind::Function => 4,
            SymbolKind::Config => 5,
            SymbolKind::Service => 6,
            SymbolKind::App => 7,
            SymbolKind::Migration => 8,
            SymbolKind::Test => 9,
            SymbolKind::Param => 10,
            SymbolKind::Variable => 11,
            SymbolKind::Field => 12,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(SymbolKind::Module),
            1 => Some(SymbolKind::Type),
            2 => Some(SymbolKind::Enum),
            3 => Some(SymbolKind::EnumVariant),
            4 => Some(SymbolKind::Function),
            5 => Some(SymbolKind::Config),
            6 => Some(SymbolKind::Service),
            7 => Some(SymbolKind::App),
            8 => Some(SymbolKind::Migration),
            9 => Some(SymbolKind::Test),
            10 => Some(SymbolKind::Param),
            11 => Some(SymbolKind::Variable),
            12 => Some(SymbolKind::Field),
            _ => None,
        }
    }

    pub(crate) fn hover_label(self) -> &'static str {
        match self {
            SymbolKind::Module => "Module",
            SymbolKind::Type => "Type",
            SymbolKind::Enum => "Enum",
            SymbolKind::EnumVariant => "Enum Variant",
            SymbolKind::Function => "Function",
            SymbolKind::Config => "Config",
            SymbolKind::Service => "Service",
            SymbolKind::App => "App",
            SymbolKind::Migration => "Migration",
            SymbolKind::Test => "Test",
            SymbolKind::Param => "Parameter",
            SymbolKind::Variable => "Variable",
            SymbolKind::Field => "Field",
        }
    }
}

pub(crate) fn span_contains(span: Span, offset: usize) -> bool {
    offset >= span.start && offset <= span.end
}

pub(crate) struct QualifiedNameRef {
    pub(crate) span: Span,
    pub(crate) module: String,
    pub(crate) item: String,
}

pub(crate) fn collect_qualified_refs(program: &Program) -> Vec<QualifiedNameRef> {
    let mut out = Vec::new();
    for item in &program.items {
        match item {
            Item::Type(decl) => {
                for field in &decl.fields {
                    collect_qualified_type_ref(&field.ty, &mut out);
                }
            }
            Item::Enum(decl) => {
                for variant in &decl.variants {
                    for ty in &variant.payload {
                        collect_qualified_type_ref(ty, &mut out);
                    }
                }
            }
            Item::Fn(decl) => {
                for param in &decl.params {
                    collect_qualified_type_ref(&param.ty, &mut out);
                }
                if let Some(ret) = &decl.ret {
                    collect_qualified_type_ref(ret, &mut out);
                }
                collect_qualified_block(&decl.body, &mut out);
            }
            Item::Service(decl) => {
                for route in &decl.routes {
                    collect_qualified_type_ref(&route.ret_type, &mut out);
                    if let Some(body) = &route.body_type {
                        collect_qualified_type_ref(body, &mut out);
                    }
                    collect_qualified_block(&route.body, &mut out);
                }
            }
            Item::Config(decl) => {
                for field in &decl.fields {
                    collect_qualified_type_ref(&field.ty, &mut out);
                    collect_qualified_expr(&field.value, &mut out);
                }
            }
            Item::Component(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::App(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::Migration(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::Test(decl) => collect_qualified_block(&decl.body, &mut out),
            Item::Import(_) => {}
        }
    }
    out
}

fn collect_qualified_block(block: &Block, out: &mut Vec<QualifiedNameRef>) {
    for stmt in &block.stmts {
        collect_qualified_stmt(stmt, out);
    }
}

fn collect_qualified_stmt(stmt: &Stmt, out: &mut Vec<QualifiedNameRef>) {
    match &stmt.kind {
        StmtKind::Let { ty, expr, .. } | StmtKind::Var { ty, expr, .. } => {
            if let Some(ty) = ty {
                collect_qualified_type_ref(ty, out);
            }
            collect_qualified_expr(expr, out);
        }
        StmtKind::Assign { target, expr } => {
            collect_qualified_expr(target, out);
            collect_qualified_expr(expr, out);
        }
        StmtKind::Return { expr } => {
            if let Some(expr) = expr {
                collect_qualified_expr(expr, out);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            collect_qualified_expr(cond, out);
            collect_qualified_block(then_block, out);
            for (cond, block) in else_if {
                collect_qualified_expr(cond, out);
                collect_qualified_block(block, out);
            }
            if let Some(block) = else_block {
                collect_qualified_block(block, out);
            }
        }
        StmtKind::Match { expr, cases } => {
            collect_qualified_expr(expr, out);
            for (pat, block) in cases {
                collect_qualified_pattern(pat, out);
                collect_qualified_block(block, out);
            }
        }
        StmtKind::For { pat, iter, block } => {
            collect_qualified_pattern(pat, out);
            collect_qualified_expr(iter, out);
            collect_qualified_block(block, out);
        }
        StmtKind::While { cond, block } => {
            collect_qualified_expr(cond, out);
            collect_qualified_block(block, out);
        }
        StmtKind::Transaction { block } => collect_qualified_block(block, out),
        StmtKind::Expr(expr) => collect_qualified_expr(expr, out),
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn collect_qualified_expr(expr: &Expr, out: &mut Vec<QualifiedNameRef>) {
    match &expr.kind {
        ExprKind::Literal(_) => {}
        ExprKind::Ident(_) => {}
        ExprKind::Binary { left, right, .. } => {
            collect_qualified_expr(left, out);
            collect_qualified_expr(right, out);
        }
        ExprKind::Unary { expr, .. } => collect_qualified_expr(expr, out),
        ExprKind::Call { callee, args } => {
            collect_qualified_expr(callee, out);
            for arg in args {
                collect_qualified_expr(&arg.value, out);
            }
        }
        ExprKind::Member { base, name } => {
            if let ExprKind::Ident(ident) = &base.kind {
                if let Some((module, item)) =
                    split_qualified_name(&format!("{}.{}", ident.name, name.name))
                {
                    out.push(QualifiedNameRef {
                        span: name.span,
                        module: module.to_string(),
                        item: item.to_string(),
                    });
                }
            }
            collect_qualified_expr(base, out);
        }
        ExprKind::OptionalMember { base, name } => {
            if let ExprKind::Ident(ident) = &base.kind {
                if let Some((module, item)) =
                    split_qualified_name(&format!("{}.{}", ident.name, name.name))
                {
                    out.push(QualifiedNameRef {
                        span: name.span,
                        module: module.to_string(),
                        item: item.to_string(),
                    });
                }
            }
            collect_qualified_expr(base, out);
        }
        ExprKind::StructLit { name, fields } => {
            if let Some((module, item)) = split_qualified_name(&name.name) {
                out.push(QualifiedNameRef {
                    span: name.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for field in fields {
                collect_qualified_expr(&field.value, out);
            }
        }
        ExprKind::ListLit(items) => {
            for item in items {
                collect_qualified_expr(item, out);
            }
        }
        ExprKind::MapLit(items) => {
            for (key, value) in items {
                collect_qualified_expr(key, out);
                collect_qualified_expr(value, out);
            }
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            collect_qualified_expr(base, out);
            collect_qualified_expr(index, out);
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let fusec::ast::InterpPart::Expr(expr) = part {
                    collect_qualified_expr(expr, out);
                }
            }
        }
        ExprKind::Coalesce { left, right } => {
            collect_qualified_expr(left, out);
            collect_qualified_expr(right, out);
        }
        ExprKind::BangChain { expr, error } => {
            collect_qualified_expr(expr, out);
            if let Some(err) = error {
                collect_qualified_expr(err, out);
            }
        }
        ExprKind::Spawn { block } => collect_qualified_block(block, out),
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            collect_qualified_expr(cond, out);
            for child in then_children {
                collect_qualified_expr(child, out);
            }
            for (branch_cond, branch_children) in else_if {
                collect_qualified_expr(branch_cond, out);
                for child in branch_children {
                    collect_qualified_expr(child, out);
                }
            }
            for child in else_children {
                collect_qualified_expr(child, out);
            }
        }
        ExprKind::HtmlFor {
            pat,
            iter,
            body_children,
        } => {
            collect_qualified_pattern(pat, out);
            collect_qualified_expr(iter, out);
            for child in body_children {
                collect_qualified_expr(child, out);
            }
        }
        ExprKind::Await { expr } => collect_qualified_expr(expr, out),
        ExprKind::Box { expr } => collect_qualified_expr(expr, out),
    }
}

fn collect_qualified_pattern(pattern: &Pattern, out: &mut Vec<QualifiedNameRef>) {
    match &pattern.kind {
        PatternKind::Wildcard | PatternKind::Literal(_) => {}
        PatternKind::Ident(_) => {}
        PatternKind::EnumVariant { name, args } => {
            if let Some((module, item)) = split_qualified_name(&name.name) {
                out.push(QualifiedNameRef {
                    span: name.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for arg in args {
                collect_qualified_pattern(arg, out);
            }
        }
        PatternKind::Struct { name, fields } => {
            if let Some((module, item)) = split_qualified_name(&name.name) {
                out.push(QualifiedNameRef {
                    span: name.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for field in fields {
                collect_qualified_pattern(&field.pat, out);
            }
        }
    }
}

fn collect_qualified_type_ref(ty: &TypeRef, out: &mut Vec<QualifiedNameRef>) {
    match &ty.kind {
        TypeRefKind::Simple(ident) => {
            if let Some((module, item)) = split_qualified_name(&ident.name) {
                out.push(QualifiedNameRef {
                    span: ident.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
        }
        TypeRefKind::Generic { base, args } => {
            if let Some((module, item)) = split_qualified_name(&base.name) {
                out.push(QualifiedNameRef {
                    span: base.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for arg in args {
                collect_qualified_type_ref(arg, out);
            }
        }
        TypeRefKind::Optional(inner) => collect_qualified_type_ref(inner, out),
        TypeRefKind::Result { ok, err } => {
            collect_qualified_type_ref(ok, out);
            if let Some(err) = err {
                collect_qualified_type_ref(err, out);
            }
        }
        TypeRefKind::Refined { base, args } => {
            if let Some((module, item)) = split_qualified_name(&base.name) {
                out.push(QualifiedNameRef {
                    span: base.span,
                    module: module.to_string(),
                    item: item.to_string(),
                });
            }
            for arg in args {
                collect_qualified_expr(arg, out);
            }
        }
    }
}

fn split_qualified_name(name: &str) -> Option<(&str, &str)> {
    let mut iter = name.rsplitn(2, '.');
    let item = iter.next()?;
    let module = iter.next()?;
    if module.is_empty() || item.is_empty() {
        return None;
    }
    Some((module, item))
}

pub(crate) struct IndexBuilder<'a> {
    text: &'a str,
    defs: Vec<SymbolDef>,
    refs: Vec<SymbolRef>,
    calls: Vec<CallRef>,
    qualified_calls: Vec<QualifiedCallRef>,
    scopes: Vec<HashMap<String, usize>>,
    globals: HashMap<String, usize>,
    app_defs: HashMap<String, usize>,
    migration_defs: HashMap<String, usize>,
    test_defs: HashMap<String, usize>,
    type_defs: HashMap<String, usize>,
    enum_variants: HashMap<String, usize>,
    enum_variant_ambiguous: HashSet<String>,
    enum_variants_by_enum: HashMap<String, HashMap<String, usize>>,
    current_callable: Option<usize>,
}

impl<'a> IndexBuilder<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self {
            text,
            defs: Vec::new(),
            refs: Vec::new(),
            calls: Vec::new(),
            qualified_calls: Vec::new(),
            scopes: Vec::new(),
            globals: HashMap::new(),
            app_defs: HashMap::new(),
            migration_defs: HashMap::new(),
            test_defs: HashMap::new(),
            type_defs: HashMap::new(),
            enum_variants: HashMap::new(),
            enum_variant_ambiguous: HashSet::new(),
            enum_variants_by_enum: HashMap::new(),
            current_callable: None,
        }
    }

    pub(crate) fn finish(self) -> Index {
        Index {
            defs: self.defs,
            refs: self.refs,
            calls: self.calls,
            qualified_calls: self.qualified_calls,
        }
    }

    pub(crate) fn collect(&mut self, program: &Program) {
        self.collect_globals(program);
        for item in &program.items {
            self.visit_item(item);
        }
    }

    fn collect_globals(&mut self, program: &Program) {
        for item in &program.items {
            match item {
                Item::Import(decl) => self.define_import(decl),
                Item::Type(decl) => self.define_type(decl),
                Item::Enum(decl) => self.define_enum(decl),
                Item::Fn(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Function,
                        self.fn_signature(decl),
                        decl.doc.as_ref(),
                        None,
                    );
                }
                Item::Config(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Config,
                        format!("config {}", decl.name.name),
                        decl.doc.as_ref(),
                        None,
                    );
                }
                Item::Service(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Service,
                        format!("service {}", decl.name.name),
                        decl.doc.as_ref(),
                        None,
                    );
                }
                Item::App(decl) => {
                    let detail = format!("app \"{}\"", decl.name.value);
                    let def_id = self.define_literal_decl(
                        &decl.name,
                        SymbolKind::App,
                        detail,
                        decl.doc.as_ref(),
                    );
                    self.app_defs.insert(decl.name.value.clone(), def_id);
                }
                Item::Migration(decl) => {
                    let detail = format!("migration {}", decl.name);
                    let def_id = self.define_span_decl(
                        decl.span,
                        decl.name.clone(),
                        SymbolKind::Migration,
                        detail,
                        decl.doc.as_ref(),
                    );
                    self.migration_defs.insert(decl.name.clone(), def_id);
                }
                Item::Test(decl) => {
                    let detail = format!("test \"{}\"", decl.name.value);
                    let def_id = self.define_literal_decl(
                        &decl.name,
                        SymbolKind::Test,
                        detail,
                        decl.doc.as_ref(),
                    );
                    self.test_defs.insert(decl.name.value.clone(), def_id);
                }
                Item::Component(decl) => {
                    self.define_global(
                        &decl.name,
                        SymbolKind::Function,
                        format!("component {}", decl.name.name),
                        decl.doc.as_ref(),
                        None,
                    );
                }
            }
        }
    }

    fn define_import(&mut self, decl: &ImportDecl) {
        match &decl.spec {
            ImportSpec::Module { name } => {
                self.define_global(
                    name,
                    SymbolKind::Module,
                    format!("module {}", name.name),
                    None,
                    None,
                );
            }
            ImportSpec::ModuleFrom { name, .. } => {
                self.define_global(
                    name,
                    SymbolKind::Module,
                    format!("module {}", name.name),
                    None,
                    None,
                );
            }
            ImportSpec::AliasFrom { alias, .. } => {
                self.define_global(
                    alias,
                    SymbolKind::Module,
                    format!("module {}", alias.name),
                    None,
                    None,
                );
            }
            ImportSpec::NamedFrom { names, .. } => {
                for name in names {
                    self.define_global(
                        name,
                        SymbolKind::Variable,
                        format!("import {}", name.name),
                        None,
                        None,
                    );
                }
            }
        }
    }

    fn define_type(&mut self, decl: &TypeDecl) {
        let def_id = self.define_global(
            &decl.name,
            SymbolKind::Type,
            format!("type {}", decl.name.name),
            decl.doc.as_ref(),
            None,
        );
        self.type_defs.insert(decl.name.name.clone(), def_id);
    }

    fn define_enum(&mut self, decl: &EnumDecl) {
        let def_id = self.define_global(
            &decl.name,
            SymbolKind::Enum,
            format!("enum {}", decl.name.name),
            decl.doc.as_ref(),
            None,
        );
        self.type_defs.insert(decl.name.name.clone(), def_id);
        let mut variants = HashMap::new();
        for variant in &decl.variants {
            let detail = if variant.payload.is_empty() {
                format!("variant {}", variant.name.name)
            } else {
                let payload = variant
                    .payload
                    .iter()
                    .map(|ty| self.type_ref_text(ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("variant {}({})", variant.name.name, payload)
            };
            let def_id = self.define_span_decl(
                variant.name.span,
                variant.name.name.clone(),
                SymbolKind::EnumVariant,
                detail,
                decl.doc.as_ref(),
            );
            variants.insert(variant.name.name.clone(), def_id);
            if self.enum_variant_ambiguous.contains(&variant.name.name) {
                continue;
            }
            if self.enum_variants.contains_key(&variant.name.name) {
                self.enum_variants.remove(&variant.name.name);
                self.enum_variant_ambiguous
                    .insert(variant.name.name.clone());
            } else {
                self.enum_variants.insert(variant.name.name.clone(), def_id);
            }
        }
        self.enum_variants_by_enum
            .insert(decl.name.name.clone(), variants);
    }

    fn visit_item(&mut self, item: &Item) {
        match item {
            Item::Import(_) => {}
            Item::Type(decl) => self.visit_type_decl(decl),
            Item::Enum(decl) => self.visit_enum_decl(decl),
            Item::Fn(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.globals.get(&decl.name.name).copied();
                self.visit_fn_decl(decl);
                self.current_callable = prev;
            }
            Item::Config(decl) => self.visit_config_decl(decl),
            Item::Service(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.globals.get(&decl.name.name).copied();
                self.visit_service_decl(decl);
                self.current_callable = prev;
            }
            Item::App(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.app_defs.get(&decl.name.value).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
            Item::Migration(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.migration_defs.get(&decl.name).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
            Item::Test(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.test_defs.get(&decl.name.value).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
            Item::Component(decl) => {
                let prev = self.current_callable;
                self.current_callable = self.globals.get(&decl.name.name).copied();
                self.visit_block(&decl.body);
                self.current_callable = prev;
            }
        }
    }

    fn visit_type_decl(&mut self, decl: &TypeDecl) {
        for field in &decl.fields {
            self.visit_type_ref(&field.ty);
            if let Some(expr) = &field.default {
                self.visit_expr(expr);
            }
        }
        if let Some(TypeDerive { base, .. }) = &decl.derive {
            self.add_type_ref(base);
        }
    }

    fn visit_enum_decl(&mut self, decl: &EnumDecl) {
        for variant in &decl.variants {
            for ty in &variant.payload {
                self.visit_type_ref(ty);
            }
        }
    }

    fn visit_fn_decl(&mut self, decl: &FnDecl) {
        self.enter_scope();
        let container = self.current_container();
        for param in &decl.params {
            let detail = format!(
                "param {}: {}",
                param.name.name,
                self.type_ref_text(&param.ty)
            );
            let def_id = self.define_local(
                &param.name,
                SymbolKind::Param,
                detail,
                None,
                container.clone(),
            );
            self.insert_local(&param.name.name, def_id);
            self.visit_type_ref(&param.ty);
            if let Some(expr) = &param.default {
                self.visit_expr(expr);
            }
        }
        if let Some(ret) = &decl.ret {
            self.visit_type_ref(ret);
        }
        self.visit_block_body(&decl.body);
        self.exit_scope();
    }

    fn visit_config_decl(&mut self, decl: &ConfigDecl) {
        for field in &decl.fields {
            let detail = format!(
                "field {}: {}",
                field.name.name,
                self.type_ref_text(&field.ty)
            );
            self.define_span_decl(
                field.name.span,
                field.name.name.clone(),
                SymbolKind::Field,
                detail,
                None,
            );
            self.visit_type_ref(&field.ty);
            self.visit_expr(&field.value);
        }
    }

    fn visit_service_decl(&mut self, decl: &ServiceDecl) {
        let container = self.current_container();
        for route in &decl.routes {
            self.visit_type_ref(&route.ret_type);
            if let Some(body_ty) = &route.body_type {
                self.visit_type_ref(body_ty);
            }
            self.enter_scope();
            if let Some(body_ty) = &route.body_type {
                let detail = format!("param body: {}", self.type_ref_text(body_ty));
                let span = route.body_span.unwrap_or(body_ty.span);
                let def_id = self.define_span_decl_with_container(
                    span,
                    "body".to_string(),
                    SymbolKind::Param,
                    detail,
                    None,
                    container.clone(),
                );
                self.insert_local("body", def_id);
            }
            self.visit_block_body(&route.body);
            self.exit_scope();
        }
    }

    fn visit_block(&mut self, block: &Block) {
        self.enter_scope();
        self.visit_block_body(block);
        self.exit_scope();
    }

    fn visit_block_body(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        let container = self.current_container();
        match &stmt.kind {
            StmtKind::Let { name, ty, expr } => {
                if let Some(ty) = ty {
                    self.visit_type_ref(ty);
                }
                self.visit_expr(expr);
                let detail = match ty {
                    Some(ty) => format!("let {}: {}", name.name, self.type_ref_text(ty)),
                    None => format!("let {}", name.name),
                };
                let def_id =
                    self.define_local(name, SymbolKind::Variable, detail, None, container.clone());
                self.insert_local(&name.name, def_id);
            }
            StmtKind::Var { name, ty, expr } => {
                if let Some(ty) = ty {
                    self.visit_type_ref(ty);
                }
                self.visit_expr(expr);
                let detail = match ty {
                    Some(ty) => format!("var {}: {}", name.name, self.type_ref_text(ty)),
                    None => format!("var {}", name.name),
                };
                let def_id =
                    self.define_local(name, SymbolKind::Variable, detail, None, container.clone());
                self.insert_local(&name.name, def_id);
            }
            StmtKind::Assign { target, expr } => {
                self.visit_expr(target);
                self.visit_expr(expr);
            }
            StmtKind::Return { expr } => {
                if let Some(expr) = expr {
                    self.visit_expr(expr);
                }
            }
            StmtKind::If {
                cond,
                then_block,
                else_if,
                else_block,
            } => {
                self.visit_expr(cond);
                self.visit_block(then_block);
                for (expr, block) in else_if {
                    self.visit_expr(expr);
                    self.visit_block(block);
                }
                if let Some(block) = else_block {
                    self.visit_block(block);
                }
            }
            StmtKind::Match { expr, cases } => {
                self.visit_expr(expr);
                for (pat, block) in cases {
                    self.enter_scope();
                    self.visit_pattern(pat);
                    self.visit_block_body(block);
                    self.exit_scope();
                }
            }
            StmtKind::For { pat, iter, block } => {
                self.visit_expr(iter);
                self.enter_scope();
                self.visit_pattern(pat);
                self.visit_block_body(block);
                self.exit_scope();
            }
            StmtKind::While { cond, block } => {
                self.visit_expr(cond);
                self.visit_block(block);
            }
            StmtKind::Transaction { block } => {
                self.visit_block(block);
            }
            StmtKind::Expr(expr) => {
                self.visit_expr(expr);
            }
            StmtKind::Break | StmtKind::Continue => {}
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Literal(_) => {}
            ExprKind::Ident(ident) => {
                if let Some(def_id) = self.resolve_value(&ident.name) {
                    self.add_ref(ident.span, def_id);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            ExprKind::Unary { expr, .. } => {
                self.visit_expr(expr);
            }
            ExprKind::Call { callee, args } => {
                self.record_call(callee);
                self.visit_expr(callee);
                for arg in args {
                    if let Some(name) = &arg.name {
                        if let Some(def_id) = self.resolve_value(&name.name) {
                            self.add_ref(name.span, def_id);
                        }
                    }
                    self.visit_expr(&arg.value);
                }
            }
            ExprKind::Member { base, name } | ExprKind::OptionalMember { base, name } => {
                if let ExprKind::Ident(base_ident) = &base.kind {
                    if let Some(map) = self.enum_variants_by_enum.get(&base_ident.name) {
                        if let Some(def_id) = map.get(&name.name) {
                            self.add_ref(name.span, *def_id);
                        }
                    }
                }
                self.visit_expr(base);
            }
            ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
                self.visit_expr(base);
                self.visit_expr(index);
            }
            ExprKind::StructLit { name, fields } => {
                self.add_type_ref(name);
                for field in fields {
                    self.visit_expr(&field.value);
                }
            }
            ExprKind::ListLit(items) => {
                for item in items {
                    self.visit_expr(item);
                }
            }
            ExprKind::MapLit(items) => {
                for (key, value) in items {
                    self.visit_expr(key);
                    self.visit_expr(value);
                }
            }
            ExprKind::InterpString(parts) => {
                for part in parts {
                    if let fusec::ast::InterpPart::Expr(expr) = part {
                        self.visit_expr(expr);
                    }
                }
            }
            ExprKind::Coalesce { left, right } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            ExprKind::BangChain { expr, error } => {
                self.visit_expr(expr);
                if let Some(err) = error {
                    self.visit_expr(err);
                }
            }
            ExprKind::Spawn { block } => self.visit_block(block),
            ExprKind::HtmlIf {
                cond,
                then_children,
                else_if,
                else_children,
            } => {
                self.visit_expr(cond);
                for child in then_children {
                    self.visit_expr(child);
                }
                for (branch_cond, branch_children) in else_if {
                    self.visit_expr(branch_cond);
                    for child in branch_children {
                        self.visit_expr(child);
                    }
                }
                for child in else_children {
                    self.visit_expr(child);
                }
            }
            ExprKind::HtmlFor {
                pat,
                iter,
                body_children,
            } => {
                self.visit_expr(iter);
                self.enter_scope();
                self.visit_pattern(pat);
                for child in body_children {
                    self.visit_expr(child);
                }
                self.exit_scope();
            }
            ExprKind::Await { expr } => self.visit_expr(expr),
            ExprKind::Box { expr } => self.visit_expr(expr),
        }
    }

    fn visit_type_ref(&mut self, ty: &TypeRef) {
        match &ty.kind {
            TypeRefKind::Simple(ident) => self.add_type_ref(ident),
            TypeRefKind::Generic { base, args } => {
                self.add_type_ref(base);
                for arg in args {
                    self.visit_type_ref(arg);
                }
            }
            TypeRefKind::Optional(inner) => self.visit_type_ref(inner),
            TypeRefKind::Result { ok, err } => {
                self.visit_type_ref(ok);
                if let Some(err) = err {
                    self.visit_type_ref(err);
                }
            }
            TypeRefKind::Refined { base, args } => {
                self.add_type_ref(base);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
        }
    }

    fn visit_pattern(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Wildcard | PatternKind::Literal(_) => {}
            PatternKind::Ident(ident) => {
                let detail = format!("let {}", ident.name);
                let def_id = self.define_local(
                    ident,
                    SymbolKind::Variable,
                    detail,
                    None,
                    self.current_container(),
                );
                self.insert_local(&ident.name, def_id);
            }
            PatternKind::EnumVariant { name, args } => {
                if let Some(def_id) = self.enum_variants.get(&name.name) {
                    self.add_ref(name.span, *def_id);
                }
                for arg in args {
                    self.visit_pattern(arg);
                }
            }
            PatternKind::Struct { name, fields } => {
                self.add_type_ref(name);
                for field in fields {
                    self.visit_pattern(&field.pat);
                }
            }
        }
    }

    fn record_call(&mut self, callee: &Expr) {
        let Some(caller) = self.current_callable else {
            return;
        };
        if let Some(target) = self.call_target_local(callee) {
            self.calls.push(CallRef {
                caller,
                callee: target,
                span: callee.span,
            });
            return;
        }
        if let Some((module, item, span)) = self.call_target_qualified(callee) {
            self.qualified_calls.push(QualifiedCallRef {
                caller,
                module,
                item,
                span,
            });
        }
    }

    fn call_target_local(&self, callee: &Expr) -> Option<usize> {
        match &callee.kind {
            ExprKind::Ident(ident) => self.resolve_value(&ident.name),
            _ => None,
        }
    }

    fn call_target_qualified(&self, callee: &Expr) -> Option<(String, String, Span)> {
        let (base, name) = match &callee.kind {
            ExprKind::Member { base, name } | ExprKind::OptionalMember { base, name } => {
                (base, name)
            }
            _ => return None,
        };
        let ExprKind::Ident(base_ident) = &base.kind else {
            return None;
        };
        let Some(base_def_id) = self.resolve_value(&base_ident.name) else {
            return None;
        };
        let Some(base_def) = self.defs.get(base_def_id) else {
            return None;
        };
        if base_def.kind != SymbolKind::Module {
            return None;
        }
        Some((base_ident.name.clone(), name.name.clone(), name.span))
    }

    fn add_type_ref(&mut self, ident: &Ident) {
        if ident.name.contains('.') {
            return;
        }
        if is_builtin_type(&ident.name) {
            return;
        }
        if let Some(def_id) = self
            .type_defs
            .get(&ident.name)
            .copied()
            .or_else(|| self.globals.get(&ident.name).copied())
        {
            self.add_ref(ident.span, def_id);
        }
    }

    fn resolve_value(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some(def_id) = scope.get(name) {
                return Some(*def_id);
            }
        }
        self.globals.get(name).copied()
    }

    fn add_ref(&mut self, span: Span, target: usize) {
        self.refs.push(SymbolRef { span, target });
    }

    fn define_global(
        &mut self,
        ident: &Ident,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
        container: Option<String>,
    ) -> usize {
        if let Some(def_id) = self.globals.get(&ident.name) {
            return *def_id;
        }
        let doc = doc.cloned();
        let def_id = self.defs.len();
        self.defs.push(SymbolDef {
            name: ident.name.clone(),
            span: ident.span,
            kind,
            detail,
            doc,
            container,
        });
        self.globals.insert(ident.name.clone(), def_id);
        def_id
    }

    fn define_literal_decl(
        &mut self,
        lit: &fusec::ast::StringLit,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
    ) -> usize {
        self.define_span_decl(lit.span, lit.value.clone(), kind, detail, doc)
    }

    fn define_span_decl(
        &mut self,
        span: Span,
        name: String,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
    ) -> usize {
        self.define_span_decl_with_container(span, name, kind, detail, doc, None)
    }

    fn define_span_decl_with_container(
        &mut self,
        span: Span,
        name: String,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
        container: Option<String>,
    ) -> usize {
        let doc = doc.cloned();
        let def_id = self.defs.len();
        self.defs.push(SymbolDef {
            name,
            span,
            kind,
            detail,
            doc,
            container,
        });
        def_id
    }

    fn define_local(
        &mut self,
        ident: &Ident,
        kind: SymbolKind,
        detail: String,
        doc: Option<&Doc>,
        container: Option<String>,
    ) -> usize {
        let doc = doc.cloned();
        let def_id = self.defs.len();
        self.defs.push(SymbolDef {
            name: ident.name.clone(),
            span: ident.span,
            kind,
            detail,
            doc,
            container,
        });
        def_id
    }

    fn insert_local(&mut self, name: &str, def_id: usize) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), def_id);
        }
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    fn fn_signature(&self, decl: &FnDecl) -> String {
        let mut out = format!("fn {}(", decl.name.name);
        for (idx, param) in decl.params.iter().enumerate() {
            if idx > 0 {
                out.push_str(", ");
            }
            out.push_str(&param.name.name);
            out.push_str(": ");
            out.push_str(&self.type_ref_text(&param.ty));
        }
        out.push(')');
        if let Some(ret) = &decl.ret {
            out.push_str(" -> ");
            out.push_str(&self.type_ref_text(ret));
        }
        out
    }

    fn type_ref_text(&self, ty: &TypeRef) -> String {
        self.slice_span(ty.span).trim().to_string()
    }

    fn current_container(&self) -> Option<String> {
        let id = self.current_callable?;
        self.defs.get(id).map(|def| def.name.clone())
    }

    fn slice_span(&self, span: Span) -> String {
        self.text
            .get(span.start..span.end)
            .unwrap_or("")
            .to_string()
    }
}

fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "Int"
            | "Float"
            | "Bool"
            | "String"
            | "Bytes"
            | "Html"
            | "Id"
            | "Email"
            | "Error"
            | "List"
            | "Map"
            | "Option"
            | "Result"
    )
}
