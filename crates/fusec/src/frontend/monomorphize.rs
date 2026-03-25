use std::collections::{HashMap, HashSet};

use crate::ast::{
    Block, Expr, ExprKind, FnDecl, Ident, InterpPart, Item, Program, Stmt, StmtKind, TypeRef,
    TypeRefKind,
};
use crate::loader::{ModuleId, ModuleRegistry};

/// Run frontend monomorphization: specialise every generic function call,
/// producing concrete copies and rewriting call sites.  Iteration continues
/// until no new specialisations are needed (handles generics calling generics).
pub fn monomorphize_registry(registry: &ModuleRegistry) -> ModuleRegistry {
    let mut working = registry.clone();
    let mut already_generated: HashSet<(ModuleId, String)> = HashSet::new();
    // Seed already_generated with pre-existing non-generic functions so we
    // don't re-process them.
    for (id, unit) in &working.modules {
        for item in &unit.program.items {
            if let Item::Fn(decl) = item {
                if decl.type_params.is_empty() {
                    already_generated.insert((*id, decl.name.name.clone()));
                }
            }
        }
    }

    loop {
        let changed = collect_and_specialize(&mut working, &mut already_generated);
        if !changed {
            break;
        }
    }

    // Final pass: rewrite all call sites
    rewrite_call_sites(&mut working);

    working
}

// ---------------------------------------------------------------------------
// Index of generic functions
// ---------------------------------------------------------------------------

/// Lightweight index: (module_id, fn_name) → FnDecl for generic functions only.
struct GenericFnIndex {
    fns: HashMap<(ModuleId, String), FnDecl>,
}

impl GenericFnIndex {
    fn build(registry: &ModuleRegistry) -> Self {
        let mut fns = HashMap::new();
        for (id, unit) in &registry.modules {
            for item in &unit.program.items {
                if let Item::Fn(decl) = item {
                    if !decl.type_params.is_empty() {
                        fns.insert((*id, decl.name.name.clone()), decl.clone());
                    }
                }
            }
        }
        Self { fns }
    }

    fn get(&self, module_id: ModuleId, name: &str) -> Option<&FnDecl> {
        self.fns.get(&(module_id, name.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Phase 1: collect call sites, generate specialisations
// ---------------------------------------------------------------------------

/// Scan every module for `Call { type_args: non-empty }` where the callee
/// resolves to a generic function.  For each unseen
/// `(owner_module, mono_name)` pair, inject the specialised `FnDecl`.
///
/// Returns `true` when at least one new specialisation was generated.
fn collect_and_specialize(
    registry: &mut ModuleRegistry,
    already_generated: &mut HashSet<(ModuleId, String)>,
) -> bool {
    let index = GenericFnIndex::build(registry);

    // Collect calls: (owner_module_id, mono_name, original_decl, bindings)
    struct Spec {
        owner_module: ModuleId,
        mono_name: String,
        decl: FnDecl,
        bindings: HashMap<String, TypeRef>,
    }

    let mut new_specs: Vec<Spec> = Vec::new();
    let mut seen_in_round: HashSet<(ModuleId, String)> = HashSet::new();

    // Iterate over modules in deterministic order
    let mut ids: Vec<_> = registry.modules.keys().copied().collect();
    ids.sort_unstable();

    for caller_module_id in &ids {
        let Some(caller_unit) = registry.modules.get(caller_module_id) else {
            continue;
        };
        let caller_unit = caller_unit.clone();

        // Walk all call sites in this module and find generic calls
        let calls =
            collect_generic_calls_in_program(&caller_unit.program, *caller_module_id, &caller_unit.modules, &caller_unit.import_items, &index);

        for (owner_module, fn_name, type_args) in calls {
            let Some(generic_decl) = index.get(owner_module, &fn_name) else {
                continue;
            };
            if generic_decl.type_params.len() != type_args.len() {
                // arity mismatch – sema will report this; skip
                continue;
            }
            let type_arg_names: Vec<String> =
                type_args.iter().map(type_ref_to_name).collect();
            let mono_name = mono_fn_name(&fn_name, &type_arg_names);
            let key = (owner_module, mono_name.clone());
            if already_generated.contains(&key) || seen_in_round.contains(&key) {
                continue;
            }
            seen_in_round.insert(key.clone());

            let bindings: HashMap<String, TypeRef> = generic_decl
                .type_params
                .iter()
                .map(|tp| tp.name.name.clone())
                .zip(type_args.into_iter())
                .collect();

            new_specs.push(Spec {
                owner_module,
                mono_name,
                decl: generic_decl.clone(),
                bindings,
            });
        }
    }

    if new_specs.is_empty() {
        return false;
    }

    for spec in new_specs {
        let mono_decl = specialize_fn(&spec.decl, &spec.bindings, &spec.mono_name);
        if let Some(unit) = registry.modules.get_mut(&spec.owner_module) {
            unit.program.items.push(Item::Fn(mono_decl));
            unit.exports
                .functions
                .insert(spec.mono_name.clone());
        }
        already_generated.insert((spec.owner_module, spec.mono_name));
    }

    true
}

// ---------------------------------------------------------------------------
// Collect all generic call sites in a program
// ---------------------------------------------------------------------------

fn collect_generic_calls_in_program(
    program: &Program,
    caller_module_id: ModuleId,
    modules: &crate::loader::ModuleMap,
    import_items: &HashMap<String, crate::loader::ModuleLink>,
    index: &GenericFnIndex,
) -> Vec<(ModuleId, String, Vec<TypeRef>)> {
    let mut out = Vec::new();
    for item in &program.items {
        match item {
            Item::Fn(decl) => {
                collect_in_block(&decl.body, caller_module_id, modules, import_items, index, &mut out)
            }
            Item::Component(decl) => {
                collect_in_block(&decl.body, caller_module_id, modules, import_items, index, &mut out)
            }
            Item::Service(decl) => {
                for route in &decl.routes {
                    collect_in_block(&route.body, caller_module_id, modules, import_items, index, &mut out);
                }
            }
            Item::Config(decl) => {
                for field in &decl.fields {
                    collect_in_expr(&field.value, caller_module_id, modules, import_items, index, &mut out);
                }
            }
            Item::App(decl) => {
                collect_in_block(&decl.body, caller_module_id, modules, import_items, index, &mut out)
            }
            Item::Migration(decl) => {
                collect_in_block(&decl.body, caller_module_id, modules, import_items, index, &mut out)
            }
            Item::Test(decl) => {
                collect_in_block(&decl.body, caller_module_id, modules, import_items, index, &mut out)
            }
            Item::Impl(decl) => {
                for method in &decl.methods {
                    collect_in_block(&method.body, caller_module_id, modules, import_items, index, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

fn collect_in_block(
    block: &Block,
    caller_module_id: ModuleId,
    modules: &crate::loader::ModuleMap,
    import_items: &HashMap<String, crate::loader::ModuleLink>,
    index: &GenericFnIndex,
    out: &mut Vec<(ModuleId, String, Vec<TypeRef>)>,
) {
    for stmt in &block.stmts {
        collect_in_stmt(stmt, caller_module_id, modules, import_items, index, out);
    }
}

fn collect_in_stmt(
    stmt: &Stmt,
    caller_module_id: ModuleId,
    modules: &crate::loader::ModuleMap,
    import_items: &HashMap<String, crate::loader::ModuleLink>,
    index: &GenericFnIndex,
    out: &mut Vec<(ModuleId, String, Vec<TypeRef>)>,
) {
    match &stmt.kind {
        StmtKind::Let { expr, .. } | StmtKind::Var { expr, .. } => {
            collect_in_expr(expr, caller_module_id, modules, import_items, index, out)
        }
        StmtKind::Assign { target, expr } => {
            collect_in_expr(target, caller_module_id, modules, import_items, index, out);
            collect_in_expr(expr, caller_module_id, modules, import_items, index, out);
        }
        StmtKind::Return { expr } => {
            if let Some(e) = expr {
                collect_in_expr(e, caller_module_id, modules, import_items, index, out);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            collect_in_expr(cond, caller_module_id, modules, import_items, index, out);
            collect_in_block(then_block, caller_module_id, modules, import_items, index, out);
            for (c, b) in else_if {
                collect_in_expr(c, caller_module_id, modules, import_items, index, out);
                collect_in_block(b, caller_module_id, modules, import_items, index, out);
            }
            if let Some(b) = else_block {
                collect_in_block(b, caller_module_id, modules, import_items, index, out);
            }
        }
        StmtKind::Match { expr, cases } => {
            collect_in_expr(expr, caller_module_id, modules, import_items, index, out);
            for (_, b) in cases {
                collect_in_block(b, caller_module_id, modules, import_items, index, out);
            }
        }
        StmtKind::For { iter, block, .. } => {
            collect_in_expr(iter, caller_module_id, modules, import_items, index, out);
            collect_in_block(block, caller_module_id, modules, import_items, index, out);
        }
        StmtKind::While { cond, block } => {
            collect_in_expr(cond, caller_module_id, modules, import_items, index, out);
            collect_in_block(block, caller_module_id, modules, import_items, index, out);
        }
        StmtKind::Transaction { block } => {
            collect_in_block(block, caller_module_id, modules, import_items, index, out)
        }
        StmtKind::Expr(e) => {
            collect_in_expr(e, caller_module_id, modules, import_items, index, out)
        }
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn collect_in_expr(
    expr: &Expr,
    caller_module_id: ModuleId,
    modules: &crate::loader::ModuleMap,
    import_items: &HashMap<String, crate::loader::ModuleLink>,
    index: &GenericFnIndex,
    out: &mut Vec<(ModuleId, String, Vec<TypeRef>)>,
) {
    match &expr.kind {
        ExprKind::Call {
            callee,
            args,
            type_args,
        } => {
            // Recurse first
            collect_in_expr(callee, caller_module_id, modules, import_items, index, out);
            for arg in args {
                collect_in_expr(&arg.value, caller_module_id, modules, import_items, index, out);
            }

            if !type_args.is_empty() {
                // Resolve the callee to a (module, fn_name) pair
                if let Some((owner_module, fn_name)) =
                    resolve_callee_to_generic(callee, caller_module_id, modules, import_items, index)
                {
                    out.push((owner_module, fn_name, type_args.clone()));
                }
            }
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
        ExprKind::Binary { left, right, .. } | ExprKind::Coalesce { left, right } => {
            collect_in_expr(left, caller_module_id, modules, import_items, index, out);
            collect_in_expr(right, caller_module_id, modules, import_items, index, out);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Await { expr }
        | ExprKind::Box { expr }
        | ExprKind::BangChain { expr, .. } => {
            collect_in_expr(expr, caller_module_id, modules, import_items, index, out)
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            collect_in_expr(base, caller_module_id, modules, import_items, index, out)
        }
        ExprKind::Index { base, index: idx } | ExprKind::OptionalIndex { base, index: idx } => {
            collect_in_expr(base, caller_module_id, modules, import_items, index, out);
            collect_in_expr(idx, caller_module_id, modules, import_items, index, out);
        }
        ExprKind::StructLit { fields, .. } => {
            for f in fields {
                collect_in_expr(&f.value, caller_module_id, modules, import_items, index, out);
            }
        }
        ExprKind::ListLit(items) => {
            for e in items {
                collect_in_expr(e, caller_module_id, modules, import_items, index, out);
            }
        }
        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                collect_in_expr(k, caller_module_id, modules, import_items, index, out);
                collect_in_expr(v, caller_module_id, modules, import_items, index, out);
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    collect_in_expr(e, caller_module_id, modules, import_items, index, out);
                }
            }
        }
        ExprKind::Spawn { block } => {
            collect_in_block(block, caller_module_id, modules, import_items, index, out)
        }
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            collect_in_expr(cond, caller_module_id, modules, import_items, index, out);
            for e in then_children {
                collect_in_expr(e, caller_module_id, modules, import_items, index, out);
            }
            for (c, children) in else_if {
                collect_in_expr(c, caller_module_id, modules, import_items, index, out);
                for e in children {
                    collect_in_expr(e, caller_module_id, modules, import_items, index, out);
                }
            }
            for e in else_children {
                collect_in_expr(e, caller_module_id, modules, import_items, index, out);
            }
        }
        ExprKind::HtmlFor {
            iter, body_children, ..
        } => {
            collect_in_expr(iter, caller_module_id, modules, import_items, index, out);
            for e in body_children {
                collect_in_expr(e, caller_module_id, modules, import_items, index, out);
            }
        }
    }
}

/// Given a callee expression, try to resolve it to the `(owner_module, fn_name)`
/// of a generic function.
fn resolve_callee_to_generic(
    callee: &Expr,
    caller_module_id: ModuleId,
    modules: &crate::loader::ModuleMap,
    import_items: &HashMap<String, crate::loader::ModuleLink>,
    index: &GenericFnIndex,
) -> Option<(ModuleId, String)> {
    match &callee.kind {
        // Plain name: `decode<T>(…)` → look up locally then via imports
        ExprKind::Ident(ident) => {
            // Could already be a canonical `mN::name` form (after previous rewrites
            // from interface desugaring – unlikely at this stage but be safe)
            if let Some((mid, name)) = parse_canonical_name(&ident.name) {
                if index.get(mid, name).is_some() {
                    return Some((mid, name.to_string()));
                }
            }
            // Local module
            if index.get(caller_module_id, &ident.name).is_some() {
                return Some((caller_module_id, ident.name.clone()));
            }
            // Imported item
            if let Some(link) = import_items.get(&ident.name) {
                if index.get(link.id, &ident.name).is_some() {
                    return Some((link.id, ident.name.clone()));
                }
            }
            None
        }
        // `Module.fn<T>(…)` – Member access on a module alias
        ExprKind::Member { base, name } => {
            if let ExprKind::Ident(base_ident) = &base.kind {
                // Try interpreting as ModuleAlias.FnName
                if let Some(link) = modules.get(&base_ident.name) {
                    if index.get(link.id, &name.name).is_some() {
                        return Some((link.id, name.name.clone()));
                    }
                }
                // Or imported item (NamedFrom brought in the module as an alias)
                if let Some(link) = import_items.get(&base_ident.name) {
                    if index.get(link.id, &name.name).is_some() {
                        return Some((link.id, name.name.clone()));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Phase 2: rewrite call sites
// ---------------------------------------------------------------------------

/// Walk every module and rewrite `Call { type_args: non-empty, callee: generic_fn }`
/// to `Call { type_args: [], callee: Ident(canonical mono name) }`.
fn rewrite_call_sites(registry: &mut ModuleRegistry) {
    // Build a fresh index of ALL functions (including newly injected mono copies)
    // We only need generic ones to know what to rewrite.
    let index = GenericFnIndex::build(registry);

    let ids: Vec<ModuleId> = {
        let mut v: Vec<_> = registry.modules.keys().copied().collect();
        v.sort_unstable();
        v
    };

    for caller_module_id in ids {
        let Some(unit) = registry.modules.get_mut(&caller_module_id) else {
            continue;
        };
        // We need immutable access to modules/import_items while mutably walking program.
        // Clone them cheaply.
        let modules = unit.modules.clone();
        let import_items = unit.import_items.clone();
        rewrite_program(
            &mut unit.program,
            caller_module_id,
            &modules,
            &import_items,
            &index,
        );
    }
}

fn rewrite_program(
    program: &mut Program,
    caller_module_id: ModuleId,
    modules: &crate::loader::ModuleMap,
    import_items: &HashMap<String, crate::loader::ModuleLink>,
    index: &GenericFnIndex,
) {
    for item in &mut program.items {
        match item {
            Item::Fn(decl) => rewrite_block(
                &mut decl.body,
                caller_module_id,
                modules,
                import_items,
                index,
            ),
            Item::Component(decl) => rewrite_block(
                &mut decl.body,
                caller_module_id,
                modules,
                import_items,
                index,
            ),
            Item::Service(decl) => {
                for route in &mut decl.routes {
                    rewrite_block(
                        &mut route.body,
                        caller_module_id,
                        modules,
                        import_items,
                        index,
                    );
                }
            }
            Item::Config(decl) => {
                for field in &mut decl.fields {
                    rewrite_expr(
                        &mut field.value,
                        caller_module_id,
                        modules,
                        import_items,
                        index,
                    );
                }
            }
            Item::App(decl) => rewrite_block(
                &mut decl.body,
                caller_module_id,
                modules,
                import_items,
                index,
            ),
            Item::Migration(decl) => rewrite_block(
                &mut decl.body,
                caller_module_id,
                modules,
                import_items,
                index,
            ),
            Item::Test(decl) => rewrite_block(
                &mut decl.body,
                caller_module_id,
                modules,
                import_items,
                index,
            ),
            Item::Impl(decl) => {
                for method in &mut decl.methods {
                    rewrite_block(
                        &mut method.body,
                        caller_module_id,
                        modules,
                        import_items,
                        index,
                    );
                }
            }
            _ => {}
        }
    }
}

fn rewrite_block(
    block: &mut Block,
    caller_module_id: ModuleId,
    modules: &crate::loader::ModuleMap,
    import_items: &HashMap<String, crate::loader::ModuleLink>,
    index: &GenericFnIndex,
) {
    for stmt in &mut block.stmts {
        rewrite_stmt(stmt, caller_module_id, modules, import_items, index);
    }
}

fn rewrite_stmt(
    stmt: &mut Stmt,
    caller_module_id: ModuleId,
    modules: &crate::loader::ModuleMap,
    import_items: &HashMap<String, crate::loader::ModuleLink>,
    index: &GenericFnIndex,
) {
    match &mut stmt.kind {
        StmtKind::Let { expr, .. } | StmtKind::Var { expr, .. } => {
            rewrite_expr(expr, caller_module_id, modules, import_items, index)
        }
        StmtKind::Assign { target, expr } => {
            rewrite_expr(target, caller_module_id, modules, import_items, index);
            rewrite_expr(expr, caller_module_id, modules, import_items, index);
        }
        StmtKind::Return { expr } => {
            if let Some(e) = expr {
                rewrite_expr(e, caller_module_id, modules, import_items, index);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            rewrite_expr(cond, caller_module_id, modules, import_items, index);
            rewrite_block(then_block, caller_module_id, modules, import_items, index);
            for (c, b) in else_if {
                rewrite_expr(c, caller_module_id, modules, import_items, index);
                rewrite_block(b, caller_module_id, modules, import_items, index);
            }
            if let Some(b) = else_block {
                rewrite_block(b, caller_module_id, modules, import_items, index);
            }
        }
        StmtKind::Match { expr, cases } => {
            rewrite_expr(expr, caller_module_id, modules, import_items, index);
            for (_, b) in cases {
                rewrite_block(b, caller_module_id, modules, import_items, index);
            }
        }
        StmtKind::For { iter, block, .. } => {
            rewrite_expr(iter, caller_module_id, modules, import_items, index);
            rewrite_block(block, caller_module_id, modules, import_items, index);
        }
        StmtKind::While { cond, block } => {
            rewrite_expr(cond, caller_module_id, modules, import_items, index);
            rewrite_block(block, caller_module_id, modules, import_items, index);
        }
        StmtKind::Transaction { block } => {
            rewrite_block(block, caller_module_id, modules, import_items, index)
        }
        StmtKind::Expr(e) => {
            rewrite_expr(e, caller_module_id, modules, import_items, index)
        }
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn rewrite_expr(
    expr: &mut Expr,
    caller_module_id: ModuleId,
    modules: &crate::loader::ModuleMap,
    import_items: &HashMap<String, crate::loader::ModuleLink>,
    index: &GenericFnIndex,
) {
    match &mut expr.kind {
        ExprKind::Call {
            callee,
            args,
            type_args,
        } => {
            // Recurse into callee and args first
            rewrite_expr(callee, caller_module_id, modules, import_items, index);
            for arg in args.iter_mut() {
                rewrite_expr(&mut arg.value, caller_module_id, modules, import_items, index);
            }

            if !type_args.is_empty() {
                if let Some((owner_module, fn_name)) = resolve_callee_to_generic(
                    callee,
                    caller_module_id,
                    modules,
                    import_items,
                    index,
                ) {
                    let decl = index.get(owner_module, &fn_name);
                    if let Some(decl) = decl {
                        if decl.type_params.len() == type_args.len() {
                            let type_arg_names: Vec<String> =
                                type_args.iter().map(type_ref_to_name).collect();
                            let mono_name = mono_fn_name(&fn_name, &type_arg_names);
                            let canonical = canonical_name(owner_module, &mono_name);
                            let span = callee.span;
                            expr.kind = ExprKind::Call {
                                callee: Box::new(Expr {
                                    kind: ExprKind::Ident(Ident {
                                        name: canonical,
                                        span,
                                    }),
                                    span,
                                }),
                                args: std::mem::take(args),
                                type_args: Vec::new(),
                            };
                            return;
                        }
                    }
                }
            }
        }
        ExprKind::Literal(_) | ExprKind::Ident(_) => {}
        ExprKind::Binary { left, right, .. } | ExprKind::Coalesce { left, right } => {
            rewrite_expr(left, caller_module_id, modules, import_items, index);
            rewrite_expr(right, caller_module_id, modules, import_items, index);
        }
        ExprKind::Unary { expr: inner, .. }
        | ExprKind::Await { expr: inner }
        | ExprKind::Box { expr: inner }
        | ExprKind::BangChain {
            expr: inner,
            error: None,
        } => rewrite_expr(inner, caller_module_id, modules, import_items, index),
        ExprKind::BangChain {
            expr: inner,
            error: Some(err),
        } => {
            rewrite_expr(inner, caller_module_id, modules, import_items, index);
            rewrite_expr(err, caller_module_id, modules, import_items, index);
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            rewrite_expr(base, caller_module_id, modules, import_items, index)
        }
        ExprKind::Index { base, index: idx } | ExprKind::OptionalIndex { base, index: idx } => {
            rewrite_expr(base, caller_module_id, modules, import_items, index);
            rewrite_expr(idx, caller_module_id, modules, import_items, index);
        }
        ExprKind::StructLit { fields, .. } => {
            for f in fields {
                rewrite_expr(&mut f.value, caller_module_id, modules, import_items, index);
            }
        }
        ExprKind::ListLit(items) => {
            for e in items {
                rewrite_expr(e, caller_module_id, modules, import_items, index);
            }
        }
        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                rewrite_expr(k, caller_module_id, modules, import_items, index);
                rewrite_expr(v, caller_module_id, modules, import_items, index);
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    rewrite_expr(e, caller_module_id, modules, import_items, index);
                }
            }
        }
        ExprKind::Spawn { block } => {
            rewrite_block(block, caller_module_id, modules, import_items, index)
        }
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            rewrite_expr(cond, caller_module_id, modules, import_items, index);
            for e in then_children {
                rewrite_expr(e, caller_module_id, modules, import_items, index);
            }
            for (c, children) in else_if {
                rewrite_expr(c, caller_module_id, modules, import_items, index);
                for e in children {
                    rewrite_expr(e, caller_module_id, modules, import_items, index);
                }
            }
            for e in else_children {
                rewrite_expr(e, caller_module_id, modules, import_items, index);
            }
        }
        ExprKind::HtmlFor {
            iter, body_children, ..
        } => {
            rewrite_expr(iter, caller_module_id, modules, import_items, index);
            for e in body_children {
                rewrite_expr(e, caller_module_id, modules, import_items, index);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Specialisation: clone + substitute
// ---------------------------------------------------------------------------

fn specialize_fn(
    decl: &FnDecl,
    bindings: &HashMap<String, TypeRef>,
    mono_name: &str,
) -> FnDecl {
    let mut new_decl = decl.clone();
    new_decl.name = Ident {
        name: mono_name.to_string(),
        span: decl.name.span,
    };
    new_decl.type_params = Vec::new();
    new_decl.where_clause = Vec::new();
    new_decl.doc = None;

    // Substitute params
    for param in &mut new_decl.params {
        subst_type_ref(&mut param.ty, bindings);
        if let Some(default) = &mut param.default {
            subst_expr(default, bindings);
        }
    }
    // Substitute return type
    if let Some(ret) = &mut new_decl.ret {
        subst_type_ref(ret, bindings);
    }
    // Substitute body
    subst_block(&mut new_decl.body, bindings);

    new_decl
}

// ---------------------------------------------------------------------------
// Type substitution
// ---------------------------------------------------------------------------

fn subst_type_ref(ty: &mut TypeRef, bindings: &HashMap<String, TypeRef>) {
    match ty.kind.clone() {
        TypeRefKind::Simple(ident) => {
            if let Some(replacement) = bindings.get(&ident.name) {
                *ty = replacement.clone();
            }
        }
        TypeRefKind::Generic { base, mut args } => {
            for arg in &mut args {
                subst_type_ref(arg, bindings);
            }
            ty.kind = TypeRefKind::Generic { base, args };
        }
        TypeRefKind::Optional(mut inner) => {
            subst_type_ref(&mut inner, bindings);
            ty.kind = TypeRefKind::Optional(inner);
        }
        TypeRefKind::Result { mut ok, mut err } => {
            subst_type_ref(&mut ok, bindings);
            if let Some(e) = &mut err {
                subst_type_ref(e, bindings);
            }
            ty.kind = TypeRefKind::Result { ok, err };
        }
        TypeRefKind::Refined { base, args } => {
            if let Some(replacement) = bindings.get(&base.name) {
                if let TypeRefKind::Simple(new_base) = &replacement.kind {
                    ty.kind = TypeRefKind::Refined {
                        base: new_base.clone(),
                        args,
                    };
                }
            } else {
                ty.kind = TypeRefKind::Refined { base, args };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Expression substitution (type param names used as values, e.g. T.method())
// ---------------------------------------------------------------------------

fn subst_block(block: &mut Block, bindings: &HashMap<String, TypeRef>) {
    for stmt in &mut block.stmts {
        subst_stmt(stmt, bindings);
    }
}

fn subst_stmt(stmt: &mut Stmt, bindings: &HashMap<String, TypeRef>) {
    match &mut stmt.kind {
        StmtKind::Let { ty, expr, .. } | StmtKind::Var { ty, expr, .. } => {
            if let Some(t) = ty {
                subst_type_ref(t, bindings);
            }
            subst_expr(expr, bindings);
        }
        StmtKind::Assign { target, expr } => {
            subst_expr(target, bindings);
            subst_expr(expr, bindings);
        }
        StmtKind::Return { expr } => {
            if let Some(e) = expr {
                subst_expr(e, bindings);
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        } => {
            subst_expr(cond, bindings);
            subst_block(then_block, bindings);
            for (c, b) in else_if {
                subst_expr(c, bindings);
                subst_block(b, bindings);
            }
            if let Some(b) = else_block {
                subst_block(b, bindings);
            }
        }
        StmtKind::Match { expr, cases } => {
            subst_expr(expr, bindings);
            for (_, b) in cases {
                subst_block(b, bindings);
            }
        }
        StmtKind::For { iter, block, .. } => {
            subst_expr(iter, bindings);
            subst_block(block, bindings);
        }
        StmtKind::While { cond, block } => {
            subst_expr(cond, bindings);
            subst_block(block, bindings);
        }
        StmtKind::Transaction { block } => subst_block(block, bindings),
        StmtKind::Expr(e) => subst_expr(e, bindings),
        StmtKind::Break | StmtKind::Continue => {}
    }
}

fn subst_expr(expr: &mut Expr, bindings: &HashMap<String, TypeRef>) {
    match &mut expr.kind {
        ExprKind::Ident(ident) => {
            // Type param used as a value (e.g. `T.decode(…)` where T is the base
            // of a member access – but also bare `T` as an expression).
            if let Some(replacement) = bindings.get(&ident.name) {
                if let TypeRefKind::Simple(new_ident) = &replacement.kind {
                    ident.name = new_ident.name.clone();
                }
            }
        }
        ExprKind::Literal(_) => {}
        ExprKind::Binary { left, right, .. } | ExprKind::Coalesce { left, right } => {
            subst_expr(left, bindings);
            subst_expr(right, bindings);
        }
        ExprKind::Unary { expr: inner, .. }
        | ExprKind::Await { expr: inner }
        | ExprKind::Box { expr: inner }
        | ExprKind::BangChain {
            expr: inner,
            error: None,
        } => subst_expr(inner, bindings),
        ExprKind::BangChain {
            expr: inner,
            error: Some(err),
        } => {
            subst_expr(inner, bindings);
            subst_expr(err, bindings);
        }
        ExprKind::Call {
            callee,
            args,
            type_args,
        } => {
            subst_expr(callee, bindings);
            for arg in args.iter_mut() {
                subst_expr(&mut arg.value, bindings);
            }
            for ta in type_args.iter_mut() {
                subst_type_ref(ta, bindings);
            }
        }
        ExprKind::Member { base, .. } | ExprKind::OptionalMember { base, .. } => {
            subst_expr(base, bindings);
        }
        ExprKind::Index { base, index } | ExprKind::OptionalIndex { base, index } => {
            subst_expr(base, bindings);
            subst_expr(index, bindings);
        }
        ExprKind::StructLit { name, fields } => {
            // If the struct name is a type param, replace it
            if let Some(replacement) = bindings.get(&name.name) {
                if let TypeRefKind::Simple(new_name) = &replacement.kind {
                    name.name = new_name.name.clone();
                }
            }
            for f in fields {
                subst_expr(&mut f.value, bindings);
            }
        }
        ExprKind::ListLit(items) => {
            for e in items {
                subst_expr(e, bindings);
            }
        }
        ExprKind::MapLit(pairs) => {
            for (k, v) in pairs {
                subst_expr(k, bindings);
                subst_expr(v, bindings);
            }
        }
        ExprKind::InterpString(parts) => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    subst_expr(e, bindings);
                }
            }
        }
        ExprKind::Spawn { block } => subst_block(block, bindings),
        ExprKind::HtmlIf {
            cond,
            then_children,
            else_if,
            else_children,
        } => {
            subst_expr(cond, bindings);
            for e in then_children {
                subst_expr(e, bindings);
            }
            for (c, children) in else_if {
                subst_expr(c, bindings);
                for e in children {
                    subst_expr(e, bindings);
                }
            }
            for e in else_children {
                subst_expr(e, bindings);
            }
        }
        ExprKind::HtmlFor {
            iter, body_children, ..
        } => {
            subst_expr(iter, bindings);
            for e in body_children {
                subst_expr(e, bindings);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Naming helpers
// ---------------------------------------------------------------------------

fn type_ref_to_name(ty: &TypeRef) -> String {
    match &ty.kind {
        TypeRefKind::Simple(ident) => ident.name.replace('.', "_"),
        TypeRefKind::Generic { base, args } => {
            format!(
                "{}_{}",
                base.name,
                args.iter()
                    .map(type_ref_to_name)
                    .collect::<Vec<_>>()
                    .join("_")
            )
        }
        TypeRefKind::Optional(inner) => format!("Opt_{}", type_ref_to_name(inner)),
        TypeRefKind::Result { ok, err } => {
            let err_str = err
                .as_deref()
                .map(type_ref_to_name)
                .unwrap_or_else(|| "Err".to_string());
            format!("Result_{}_{}", type_ref_to_name(ok), err_str)
        }
        TypeRefKind::Refined { base, .. } => base.name.clone(),
    }
}

fn mono_fn_name(fn_name: &str, type_arg_names: &[String]) -> String {
    format!("{}_{}", fn_name, type_arg_names.join("_"))
}

fn canonical_name(module_id: ModuleId, name: &str) -> String {
    format!("m{module_id}::{name}")
}

fn parse_canonical_name(name: &str) -> Option<(ModuleId, &str)> {
    let rest = name.strip_prefix('m')?;
    let (module_id, raw_name) = rest.split_once("::")?;
    Some((module_id.parse().ok()?, raw_name))
}
