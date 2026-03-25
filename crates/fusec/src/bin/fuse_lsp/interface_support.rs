use std::collections::HashSet;

use fusec::ast::{FnDecl, Ident, InterfaceDecl, InterfaceMember, Item, Program, TypeParam, WhereConstraint};
use fusec::parse_source;

use super::super::{SymbolKind, line_offsets, offset_to_line_col};
use super::workspace::WorkspaceIndex;

#[derive(Clone)]
pub(crate) struct ResolvedInterfaceDecl {
    pub(crate) text: String,
    pub(crate) decl: InterfaceDecl,
}

pub(crate) fn resolve_interface_decl(
    index: Option<&WorkspaceIndex>,
    current_uri: &str,
    current_text: &str,
    current_program: &Program,
    ident: &Ident,
) -> Option<ResolvedInterfaceDecl> {
    if let Some(decl) = find_interface_decl(current_program, &ident.name) {
        return Some(ResolvedInterfaceDecl {
            text: current_text.to_string(),
            decl,
        });
    }
    let index = index?;
    let offsets = line_offsets(current_text);
    let (line, character) = offset_to_line_col(&offsets, ident.span.start);
    let def = index.definition_at(current_uri, line, character)?;
    if def.def.kind != SymbolKind::Interface {
        return None;
    }
    let text = index.file_text(&def.uri)?.to_string();
    let (program, _parse_diags) = parse_source(&text);
    let decl = find_interface_decl(&program, &def.def.name)?;
    Some(ResolvedInterfaceDecl { text, decl })
}

pub(crate) fn collect_workspace_impl_pairs(index: &WorkspaceIndex) -> HashSet<(String, String)> {
    let mut out = HashSet::new();
    for file in &index.files {
        let (program, _parse_diags) = parse_source(&file.text);
        for item in &program.items {
            if let Item::Impl(decl) = item {
                out.insert((decl.interface.name.clone(), decl.target.name.clone()));
            }
        }
    }
    out
}

pub(crate) fn render_impl_method_signature(decl: &FnDecl, text: &str) -> String {
    render_signature(
        &decl.name.name,
        type_param_list(&decl.type_params),
        decl.params
            .iter()
            .map(|param| (param.name.name.as_str(), slice_span(text, param.ty.span)))
            .collect(),
        decl.ret.as_ref().map(|ty| slice_span(text, ty.span)),
        where_clause_text(&decl.where_clause),
    )
}

pub(crate) fn render_interface_member_signature(member: &InterfaceMember, text: &str) -> String {
    render_signature(
        &member.name.name,
        type_param_list(&member.type_params),
        member
            .params
            .iter()
            .map(|param| (param.name.name.as_str(), slice_span(text, param.ty.span)))
            .collect(),
        member.ret.as_ref().map(|ty| slice_span(text, ty.span)),
        where_clause_text(&member.where_clause),
    )
}

pub(crate) fn render_impl_method_param_labels(decl: &FnDecl, text: &str) -> Vec<String> {
    decl.params
        .iter()
        .map(|param| format!("{}: {}", param.name.name, slice_span(text, param.ty.span)))
        .collect()
}



pub(crate) fn render_interface_member_stub(
    member: &InterfaceMember,
    text: &str,
    indent: &str,
) -> String {
    let body_indent = format!("{indent}  ");
    format!(
        "{indent}{}:\n{body_indent}assert(false, \"TODO: implement {}\")\n",
        render_interface_member_signature(member, text),
        member.name.name,
    )
}

pub(crate) fn render_impl_skeleton(
    interface_name: &str,
    target_name: &str,
    members: &[InterfaceMember],
    text: &str,
) -> String {
    let mut out = format!("impl {interface_name} for {target_name}:\n");
    for member in members {
        out.push_str(&render_interface_member_stub(member, text, "  "));
    }
    out
}

fn find_interface_decl(program: &Program, name: &str) -> Option<InterfaceDecl> {
    program.items.iter().find_map(|item| match item {
        Item::Interface(decl) if decl.name.name == name => Some(decl.clone()),
        _ => None,
    })
}

fn type_param_list(type_params: &[TypeParam]) -> Option<String> {
    if type_params.is_empty() {
        return None;
    }
    let names: Vec<&str> = type_params.iter().map(|tp| tp.name.name.as_str()).collect();
    Some(format!("<{}>", names.join(", ")))
}

fn where_clause_text(where_clause: &[WhereConstraint]) -> Option<String> {
    if where_clause.is_empty() {
        return None;
    }
    let parts: Vec<String> = where_clause
        .iter()
        .map(|c| format!("{}: {}", c.type_param.name, c.interface.name))
        .collect();
    Some(format!(" where {}", parts.join(", ")))
}

fn render_signature(
    name: &str,
    type_params: Option<String>,
    params: Vec<(&str, String)>,
    ret: Option<String>,
    where_clause: Option<String>,
) -> String {
    let mut out = format!("fn {name}");
    if let Some(tps) = type_params {
        out.push_str(&tps);
    }
    out.push('(');
    for (idx, (param_name, ty)) in params.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(param_name);
        out.push_str(": ");
        out.push_str(ty.trim());
    }
    out.push(')');
    if let Some(ret) = ret {
        if !ret.trim().is_empty() {
            out.push_str(" -> ");
            out.push_str(ret.trim());
        }
    }
    if let Some(wc) = where_clause {
        out.push_str(&wc);
    }
    out
}

fn slice_span(text: &str, span: fusec::span::Span) -> String {
    text.get(span.start..span.end)
        .unwrap_or("")
        .trim()
        .to_string()
}
