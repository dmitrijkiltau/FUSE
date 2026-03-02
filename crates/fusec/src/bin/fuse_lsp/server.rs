use std::io::{self, Read, Write};

use fuse_rt::json::{self, JsonValue};

use super::super::{
    LspState, apply_doc_overlay_change, cancelled_error, capabilities_result, extract_change_text,
    extract_root_uri, extract_text_doc_text, extract_text_doc_uri, get_string, handle_cancel,
    json_error_response, json_response, read_message, workspace_stats_result, write_message,
};

pub(crate) fn run(
    stdin: &mut impl Read,
    stdout: &mut impl Write,
    state: &mut LspState,
) -> io::Result<()> {
    let mut shutdown = false;

    loop {
        let message = match read_message(stdin)? {
            Some(value) => value,
            None => break,
        };
        let value = match json::decode(&message) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let JsonValue::Object(obj) = value else {
            continue;
        };
        let method = get_string(&obj, "method");
        let id = obj.get("id").cloned();

        if method.as_deref() == Some("$/cancelRequest") {
            handle_cancel(state, &obj);
            continue;
        }

        if let Some(err) = cancelled_error(state, id.as_ref()) {
            if id.is_some() {
                let response = json_error_response(id, -32800, &err);
                write_message(stdout, &response)?;
            }
            continue;
        }

        match method.as_deref() {
            Some("initialize") => {
                state.root_uri = extract_root_uri(&obj);
                state.invalidate_workspace_cache();
                let result = capabilities_result();
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("initialized") => {}
            Some("shutdown") => {
                shutdown = true;
                let response = json_response(id, JsonValue::Null);
                write_message(stdout, &response)?;
            }
            Some("exit") => {
                if shutdown {
                    break;
                } else {
                    std::process::exit(1);
                }
            }
            Some("textDocument/didOpen") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = extract_text_doc_text(&obj) {
                        apply_doc_overlay_change(state, &uri, Some(text.clone()));
                        super::diagnostics::publish_diagnostics(stdout, state, &uri, &text)?;
                    }
                }
            }
            Some("textDocument/didChange") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = extract_change_text(&obj) {
                        apply_doc_overlay_change(state, &uri, Some(text.clone()));
                        super::diagnostics::publish_diagnostics(stdout, state, &uri, &text)?;
                    }
                }
            }
            Some("textDocument/didClose") => {
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    apply_doc_overlay_change(state, &uri, None);
                    super::diagnostics::publish_empty_diagnostics(stdout, &uri)?;
                }
            }
            Some("textDocument/formatting") => {
                let mut edits = Vec::new();
                if let Some(uri) = extract_text_doc_uri(&obj) {
                    if let Some(text) = state.docs.get(&uri).cloned() {
                        let formatted = fusec::format::format_source(&text);
                        if formatted != text {
                            edits.push(super::super::full_document_edit(&text, &formatted));
                            apply_doc_overlay_change(state, &uri, Some(formatted));
                        }
                    }
                }
                let response = json_response(id, JsonValue::Array(edits));
                write_message(stdout, &response)?;
            }
            Some("textDocument/definition") => {
                let result = super::navigation::handle_definition(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/hover") => {
                let result = super::navigation::handle_hover(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/signatureHelp") => {
                let result = super::completion::handle_signature_help(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/completion") => {
                let result = super::completion::handle_completion(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/rename") => {
                let result = super::refactor::handle_rename(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/prepareRename") => {
                let result = super::refactor::handle_prepare_rename(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/references") => {
                let result = super::navigation::handle_references(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/prepareCallHierarchy") => {
                let result = super::navigation::handle_prepare_call_hierarchy(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("callHierarchy/incomingCalls") => {
                let result = super::navigation::handle_call_hierarchy_incoming(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("callHierarchy/outgoingCalls") => {
                let result = super::navigation::handle_call_hierarchy_outgoing(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("workspace/symbol") => {
                let result = super::refactor::handle_workspace_symbol(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/codeAction") => {
                let result = super::refactor::handle_code_action(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/semanticTokens/full") => {
                let result = super::tokens::handle_semantic_tokens(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/semanticTokens/range") => {
                let result = super::tokens::handle_semantic_tokens_range(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("textDocument/inlayHint") => {
                let result = super::tokens::handle_inlay_hints(state, &obj);
                let response = json_response(id, result);
                write_message(stdout, &response)?;
            }
            Some("fuse/internalWorkspaceStats") => {
                let response = json_response(id, workspace_stats_result(state));
                write_message(stdout, &response)?;
            }
            _ => {
                if id.is_some() {
                    let response = json_response(id, JsonValue::Null);
                    write_message(stdout, &response)?;
                }
            }
        }
    }
    Ok(())
}
