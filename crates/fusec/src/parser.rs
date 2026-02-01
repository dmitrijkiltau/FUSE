use crate::ast::*;
use crate::diag::Diagnostics;
use crate::lexer;
use crate::span::Span;
use crate::token::{InterpSegment, Keyword, Punct, Token, TokenKind};

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    diags: &'a mut Diagnostics,
}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token], diags: &'a mut Diagnostics) -> Self {
        Self { tokens, pos: 0, diags }
    }

    pub fn parse_program(&mut self) -> Program {
        let mut items = Vec::new();
        while !self.at_eof() {
            self.consume_newlines();
            if self.at_eof() {
                break;
            }
            let doc = self.take_doc_comments();
            if self.at_eof() {
                break;
            }
            match self.parse_item(doc) {
                Some(item) => items.push(item),
                None => self.sync_to_next_item(),
            }
        }
        Program { items }
    }

    fn parse_item(&mut self, doc: Option<Doc>) -> Option<Item> {
        if self.eat_keyword(Keyword::Import).is_some() {
            let decl = self.parse_import_decl();
            return Some(Item::Import(decl));
        }
        if self.eat_keyword(Keyword::Type).is_some() {
            let decl = self.parse_type_decl(doc);
            return Some(Item::Type(decl));
        }
        if self.eat_keyword(Keyword::Enum).is_some() {
            let decl = self.parse_enum_decl(doc);
            return Some(Item::Enum(decl));
        }
        if self.eat_keyword(Keyword::Fn).is_some() {
            let decl = self.parse_fn_decl(doc);
            return Some(Item::Fn(decl));
        }
        if self.eat_keyword(Keyword::Service).is_some() {
            let decl = self.parse_service_decl(doc);
            return Some(Item::Service(decl));
        }
        if self.eat_keyword(Keyword::Config).is_some() {
            let decl = self.parse_config_decl(doc);
            return Some(Item::Config(decl));
        }
        if self.eat_keyword(Keyword::App).is_some() {
            let decl = self.parse_app_decl(doc);
            return Some(Item::App(decl));
        }
        if self.eat_keyword(Keyword::Migration).is_some() {
            let decl = self.parse_migration_decl(doc);
            return Some(Item::Migration(decl));
        }
        if self.eat_keyword(Keyword::Test).is_some() {
            let decl = self.parse_test_decl(doc);
            return Some(Item::Test(decl));
        }

        self.error_here("expected a top-level declaration");
        None
    }

    fn parse_import_decl(&mut self) -> ImportDecl {
        let start = self.prev_span();
        let spec = if self.eat_punct(Punct::LBrace).is_some() {
            let mut names = Vec::new();
            if !self.at_punct(Punct::RBrace) {
                loop {
                    names.push(self.expect_ident());
                    if self.eat_punct(Punct::Comma).is_none() {
                        break;
                    }
                }
            }
            self.expect_punct(Punct::RBrace);
            self.expect_keyword(Keyword::From);
            let path = self.expect_string_lit();
            ImportSpec::NamedFrom { names, path }
        } else {
            let name = self.expect_ident();
            if self.eat_keyword(Keyword::As).is_some() {
                let alias = self.expect_ident();
                self.expect_keyword(Keyword::From);
                let path = self.expect_string_lit();
                ImportSpec::AliasFrom { name, alias, path }
            } else if self.eat_keyword(Keyword::From).is_some() {
                let path = self.expect_string_lit();
                ImportSpec::ModuleFrom { name, path }
            } else {
                ImportSpec::Module { name }
            }
        };
        let end = self.prev_span();
        ImportDecl {
            spec,
            span: start.merge(end),
        }
    }

    fn parse_type_decl(&mut self, doc: Option<Doc>) -> TypeDecl {
        let name = self.expect_ident();
        if self.eat_punct(Punct::Colon).is_some() {
            self.expect_newline();
            self.expect_indent();
            let mut fields = Vec::new();
            while !self.at_dedent() && !self.at_eof() {
                self.consume_newlines();
                if self.at_dedent() || self.at_eof() {
                    break;
                }
                let field_start = self.peek_span();
                let field_name = self.expect_ident();
                self.expect_punct(Punct::Colon);
                let ty = self.parse_type_ref();
                let default = if self.eat_punct(Punct::Assign).is_some() {
                    Some(self.parse_expr())
                } else {
                    None
                };
                self.expect_newline();
                let field_end = self.prev_span();
                fields.push(FieldDecl {
                    name: field_name,
                    ty,
                    default,
                    span: field_start.merge(field_end),
                });
            }
            let end = self.expect_dedent();
            let span = name.span.merge(end);
            TypeDecl {
                name,
                fields,
                derive: None,
                doc,
                span,
            }
        } else if self.eat_punct(Punct::Assign).is_some() {
            let base = self.parse_type_name();
            self.expect_keyword(Keyword::Without);
            let mut without = Vec::new();
            loop {
                without.push(self.expect_ident());
                if self.eat_punct(Punct::Comma).is_none() {
                    break;
                }
            }
            self.expect_newline();
            let span = name.span.merge(self.prev_span());
            TypeDecl {
                name,
                fields: Vec::new(),
                derive: Some(TypeDerive {
                    base,
                    without,
                    span,
                }),
                doc,
                span,
            }
        } else {
            self.error_here("expected ':' or '=' after type name");
            let span = name.span;
            TypeDecl {
                name,
                fields: Vec::new(),
                derive: None,
                doc,
                span,
            }
        }
    }

    fn parse_enum_decl(&mut self, doc: Option<Doc>) -> EnumDecl {
        let name = self.expect_ident();
        self.expect_punct(Punct::Colon);
        self.expect_newline();
        self.expect_indent();
        let mut variants = Vec::new();
        while !self.at_dedent() && !self.at_eof() {
            self.consume_newlines();
            if self.at_dedent() || self.at_eof() {
                break;
            }
            let start = self.peek_span();
            let variant_name = self.expect_ident();
            let mut payload = Vec::new();
            if self.eat_punct(Punct::LParen).is_some() {
                if !self.at_punct(Punct::RParen) {
                    loop {
                        payload.push(self.parse_type_ref());
                        if self.eat_punct(Punct::Comma).is_none() {
                            break;
                        }
                    }
                }
                self.expect_punct(Punct::RParen);
            }
            self.expect_newline();
            let end = self.prev_span();
            variants.push(EnumVariant {
                name: variant_name,
                payload,
                span: start.merge(end),
            });
        }
        let end = self.expect_dedent();
        let span = name.span.merge(end);
        EnumDecl {
            name,
            variants,
            doc,
            span,
        }
    }

    fn parse_fn_decl(&mut self, doc: Option<Doc>) -> FnDecl {
        let name = self.expect_ident();
        self.expect_punct(Punct::LParen);
        let mut params = Vec::new();
        if !self.at_punct(Punct::RParen) {
            loop {
                params.push(self.parse_param());
                if self.eat_punct(Punct::Comma).is_none() {
                    break;
                }
            }
        }
        self.expect_punct(Punct::RParen);
        let ret = if self.eat_punct(Punct::Arrow).is_some() {
            Some(self.parse_type_ref())
        } else {
            None
        };
        self.expect_punct(Punct::Colon);
        let body = self.parse_block();
        let span = name.span.merge(body.span);
        FnDecl {
            name,
            params,
            ret,
            body,
            doc,
            span,
        }
    }

    fn parse_service_decl(&mut self, doc: Option<Doc>) -> ServiceDecl {
        let name = self.expect_ident();
        self.expect_keyword(Keyword::At);
        let base_path = self.expect_string_lit();
        self.expect_punct(Punct::Colon);
        self.expect_newline();
        self.expect_indent();
        let mut routes = Vec::new();
        while !self.at_dedent() && !self.at_eof() {
            self.consume_newlines();
            if self.at_dedent() || self.at_eof() {
                break;
            }
            routes.push(self.parse_route_decl());
        }
        let end = self.expect_dedent();
        let span = name.span.merge(end);
        ServiceDecl {
            name,
            base_path,
            routes,
            doc,
            span,
        }
    }

    fn parse_route_decl(&mut self) -> RouteDecl {
        let start = self.peek_span();
        let verb = match self.peek_kind() {
            TokenKind::Keyword(Keyword::Get) => {
                self.bump();
                HttpVerb::Get
            }
            TokenKind::Keyword(Keyword::Post) => {
                self.bump();
                HttpVerb::Post
            }
            TokenKind::Keyword(Keyword::Put) => {
                self.bump();
                HttpVerb::Put
            }
            TokenKind::Keyword(Keyword::Patch) => {
                self.bump();
                HttpVerb::Patch
            }
            TokenKind::Keyword(Keyword::Delete) => {
                self.bump();
                HttpVerb::Delete
            }
            _ => {
                self.error_here("expected HTTP verb");
                self.bump();
                HttpVerb::Get
            }
        };
        let path = self.expect_string_lit();
        let body_type = if self.eat_keyword(Keyword::Body).is_some() {
            Some(self.parse_type_ref())
        } else {
            None
        };
        self.expect_punct(Punct::Arrow);
        let ret_type = self.parse_type_ref();
        self.expect_punct(Punct::Colon);
        let body = self.parse_block();
        let span = start.merge(body.span);
        RouteDecl {
            verb,
            path,
            body_type,
            ret_type,
            body,
            span,
        }
    }

    fn parse_config_decl(&mut self, doc: Option<Doc>) -> ConfigDecl {
        let name = self.expect_ident();
        self.expect_punct(Punct::Colon);
        self.expect_newline();
        self.expect_indent();
        let mut fields = Vec::new();
        while !self.at_dedent() && !self.at_eof() {
            self.consume_newlines();
            if self.at_dedent() || self.at_eof() {
                break;
            }
            let start = self.peek_span();
            let field_name = self.expect_ident();
            self.expect_punct(Punct::Colon);
            let ty = self.parse_type_ref();
            self.expect_punct(Punct::Assign);
            let value = self.parse_expr();
            self.expect_newline();
            let end = self.prev_span();
            fields.push(ConfigField {
                name: field_name,
                ty,
                value,
                span: start.merge(end),
            });
        }
        let end = self.expect_dedent();
        let span = name.span.merge(end);
        ConfigDecl {
            name,
            fields,
            doc,
            span,
        }
    }

    fn parse_app_decl(&mut self, doc: Option<Doc>) -> AppDecl {
        let name = self.expect_string_lit();
        self.expect_punct(Punct::Colon);
        let body = self.parse_block();
        let span = name.span.merge(body.span);
        AppDecl { name, body, doc, span }
    }

    fn parse_migration_decl(&mut self, doc: Option<Doc>) -> MigrationDecl {
        let start = self.prev_span();
        let name = match self.peek_kind() {
            TokenKind::Ident(_) => self.expect_ident().name,
            TokenKind::String(_) => self.expect_string_lit().value,
            TokenKind::Int(_) => match self.bump().kind {
                TokenKind::Int(v) => v.to_string(),
                _ => "_".to_string(),
            },
            _ => {
                self.error_here("expected migration name");
                "_".to_string()
            }
        };
        self.expect_punct(Punct::Colon);
        let body = self.parse_block();
        let span = start.merge(body.span);
        MigrationDecl {
            name,
            body,
            doc,
            span,
        }
    }

    fn parse_test_decl(&mut self, doc: Option<Doc>) -> TestDecl {
        let name = self.expect_string_lit();
        self.expect_punct(Punct::Colon);
        let body = self.parse_block();
        let span = name.span.merge(body.span);
        TestDecl { name, body, doc, span }
    }

    fn parse_param(&mut self) -> Param {
        let start = self.peek_span();
        let name = self.expect_ident();
        self.expect_punct(Punct::Colon);
        let ty = self.parse_type_ref();
        let default = if self.eat_punct(Punct::Assign).is_some() {
            Some(self.parse_expr())
        } else {
            None
        };
        let end = self.prev_span();
        Param {
            name,
            ty,
            default,
            span: start.merge(end),
        }
    }

    fn parse_block(&mut self) -> Block {
        self.expect_newline();
        let indent = self.expect_indent();
        let mut stmts = Vec::new();
        while !self.at_dedent() && !self.at_eof() {
            self.consume_newlines();
            if self.at_dedent() || self.at_eof() {
                break;
            }
            stmts.push(self.parse_stmt());
        }
        let end = self.expect_dedent();
        Block {
            stmts,
            span: indent.merge(end),
        }
    }

    fn parse_stmt(&mut self) -> Stmt {
        let start = self.peek_span();
        let kind = match self.peek_kind() {
            TokenKind::Keyword(Keyword::Let) => {
                self.bump();
                let name = self.expect_ident();
                let ty = if self.eat_punct(Punct::Colon).is_some() {
                    Some(self.parse_type_ref())
                } else {
                    None
                };
                self.expect_punct(Punct::Assign);
                let expr = self.parse_expr();
                StmtKind::Let { name, ty, expr }
            }
            TokenKind::Keyword(Keyword::Var) => {
                self.bump();
                let name = self.expect_ident();
                let ty = if self.eat_punct(Punct::Colon).is_some() {
                    Some(self.parse_type_ref())
                } else {
                    None
                };
                self.expect_punct(Punct::Assign);
                let expr = self.parse_expr();
                StmtKind::Var { name, ty, expr }
            }
            TokenKind::Keyword(Keyword::Return) => {
                self.bump();
                let expr = if self.at_newline() {
                    None
                } else {
                    Some(self.parse_expr())
                };
                StmtKind::Return { expr }
            }
            TokenKind::Keyword(Keyword::If) => self.parse_if_stmt(),
            TokenKind::Keyword(Keyword::Match) => self.parse_match_stmt(),
            TokenKind::Keyword(Keyword::For) => self.parse_for_stmt(),
            TokenKind::Keyword(Keyword::While) => self.parse_while_stmt(),
            TokenKind::Keyword(Keyword::Break) => {
                self.bump();
                StmtKind::Break
            }
            TokenKind::Keyword(Keyword::Continue) => {
                self.bump();
                StmtKind::Continue
            }
            _ => {
                let expr = self.parse_expr();
                if self.eat_punct(Punct::Assign).is_some() {
                    let value = self.parse_expr();
                    StmtKind::Assign { target: expr, expr: value }
                } else {
                    StmtKind::Expr(expr)
                }
            }
        };
        let spawn_expr = |expr: &Expr| matches!(expr.kind, ExprKind::Spawn { .. });
        let needs_newline = match &kind {
            StmtKind::If { .. }
            | StmtKind::Match { .. }
            | StmtKind::For { .. }
            | StmtKind::While { .. } => false,
            StmtKind::Expr(expr) => !spawn_expr(expr),
            StmtKind::Let { expr, .. } => !spawn_expr(expr),
            StmtKind::Var { expr, .. } => !spawn_expr(expr),
            StmtKind::Assign { expr, .. } => !spawn_expr(expr),
            StmtKind::Return { expr } => expr.as_ref().map(spawn_expr).map(|v| !v).unwrap_or(true),
            _ => true,
        };
        if needs_newline {
            self.expect_newline();
        } else {
            self.consume_newlines();
        }
        let end = self.prev_span();
        Stmt {
            kind,
            span: start.merge(end),
        }
    }

    fn parse_if_stmt(&mut self) -> StmtKind {
        self.expect_keyword(Keyword::If);
        let cond = self.parse_expr();
        self.expect_punct(Punct::Colon);
        let then_block = self.parse_block();
        let mut else_if = Vec::new();
        let mut else_block = None;
        loop {
            self.consume_newlines();
            if self.eat_keyword(Keyword::Else).is_none() {
                break;
            }
            if self.eat_keyword(Keyword::If).is_some() {
                let cond = self.parse_expr();
                self.expect_punct(Punct::Colon);
                let block = self.parse_block();
                else_if.push((cond, block));
            } else {
                self.expect_punct(Punct::Colon);
                else_block = Some(self.parse_block());
                break;
            }
        }
        StmtKind::If {
            cond,
            then_block,
            else_if,
            else_block,
        }
    }

    fn parse_match_stmt(&mut self) -> StmtKind {
        self.expect_keyword(Keyword::Match);
        let expr = self.parse_expr();
        self.expect_punct(Punct::Colon);
        self.expect_newline();
        self.expect_indent();
        let mut cases = Vec::new();
        while !self.at_dedent() && !self.at_eof() {
            self.consume_newlines();
            if self.at_dedent() || self.at_eof() {
                break;
            }
            self.expect_keyword(Keyword::Case);
            let pat = self.parse_pattern();
            self.expect_punct(Punct::Colon);
            let block = self.parse_block();
            cases.push((pat, block));
        }
        self.expect_dedent();
        StmtKind::Match { expr, cases }
    }

    fn parse_for_stmt(&mut self) -> StmtKind {
        self.expect_keyword(Keyword::For);
        let pat = self.parse_pattern();
        self.expect_keyword(Keyword::In);
        let iter = self.parse_expr();
        self.expect_punct(Punct::Colon);
        let block = self.parse_block();
        StmtKind::For { pat, iter, block }
    }

    fn parse_while_stmt(&mut self) -> StmtKind {
        self.expect_keyword(Keyword::While);
        let cond = self.parse_expr();
        self.expect_punct(Punct::Colon);
        let block = self.parse_block();
        StmtKind::While { cond, block }
    }

    fn parse_pattern(&mut self) -> Pattern {
        let start = self.peek_span();
        let kind = match self.peek_kind() {
            TokenKind::Ident(name) if name == "_" => {
                self.bump();
                PatternKind::Wildcard
            }
            TokenKind::Ident(_) | TokenKind::Keyword(Keyword::Body) => {
                let ident = self.expect_ident_or_body();
                if self.eat_punct(Punct::LParen).is_some() {
                    let mut args = Vec::new();
                    let mut fields = Vec::new();
                    let mut has_named = false;
                    let mut has_positional = false;
                    if !self.at_punct(Punct::RParen) {
                        loop {
                            if self.is_named_pattern() {
                                has_named = true;
                                let field_name = self.expect_ident();
                                self.expect_punct(Punct::Assign);
                                let pat = self.parse_pattern();
                                let span = field_name.span.merge(pat.span);
                                fields.push(PatternField {
                                    name: field_name,
                                    pat: Box::new(pat),
                                    span,
                                });
                            } else {
                                has_positional = true;
                                args.push(self.parse_pattern());
                            }
                            if self.eat_punct(Punct::Comma).is_none() {
                                break;
                            }
                        }
                    }
                    let end = self.expect_punct(Punct::RParen);
                    if has_named && has_positional {
                        self.diags.error(
                            start.merge(end),
                            "cannot mix positional and named patterns",
                        );
                    }
                    if has_named {
                        PatternKind::Struct { name: ident, fields }
                    } else {
                        PatternKind::EnumVariant { name: ident, args }
                    }
                } else {
                    PatternKind::Ident(ident)
                }
            }
            TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Bool(_)
            | TokenKind::String(_)
            | TokenKind::Null => {
                let lit = self.parse_literal();
                PatternKind::Literal(lit)
            }
            _ => {
                self.error_here("expected pattern");
                self.bump();
                PatternKind::Wildcard
            }
        };
        let end = self.prev_span();
        Pattern {
            kind,
            span: start.merge(end),
        }
    }

    fn parse_expr(&mut self) -> Expr {
        self.parse_coalesce()
    }

    fn parse_coalesce(&mut self) -> Expr {
        let mut expr = self.parse_or();
        while self.eat_punct(Punct::QuestionQuestion).is_some() {
            let right = self.parse_or();
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Coalesce {
                    left: Box::new(expr),
                    right: Box::new(right),
                },
                span,
            };
        }
        expr
    }

    fn parse_or(&mut self) -> Expr {
        let mut expr = self.parse_and();
        while self.eat_keyword(Keyword::Or).is_some() {
            let right = self.parse_and();
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary {
                    op: BinaryOp::Or,
                    left: Box::new(expr),
                    right: Box::new(right),
                },
                span,
            };
        }
        expr
    }

    fn parse_and(&mut self) -> Expr {
        let mut expr = self.parse_eq();
        while self.eat_keyword(Keyword::And).is_some() {
            let right = self.parse_eq();
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary {
                    op: BinaryOp::And,
                    left: Box::new(expr),
                    right: Box::new(right),
                },
                span,
            };
        }
        expr
    }

    fn parse_eq(&mut self) -> Expr {
        let mut expr = self.parse_rel();
        loop {
            let op = if self.eat_punct(Punct::EqEq).is_some() {
                Some(BinaryOp::Eq)
            } else if self.eat_punct(Punct::NotEq).is_some() {
                Some(BinaryOp::NotEq)
            } else {
                None
            };
            if let Some(op) = op {
                let right = self.parse_rel();
                let span = expr.span.merge(right.span);
                expr = Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(expr),
                        right: Box::new(right),
                    },
                    span,
                };
            } else {
                break;
            }
        }
        expr
    }

    fn parse_rel(&mut self) -> Expr {
        let mut expr = self.parse_range();
        loop {
            let op = if self.eat_punct(Punct::Lt).is_some() {
                Some(BinaryOp::Lt)
            } else if self.eat_punct(Punct::LtEq).is_some() {
                Some(BinaryOp::LtEq)
            } else if self.eat_punct(Punct::Gt).is_some() {
                Some(BinaryOp::Gt)
            } else if self.eat_punct(Punct::GtEq).is_some() {
                Some(BinaryOp::GtEq)
            } else {
                None
            };
            if let Some(op) = op {
                let right = self.parse_range();
                let span = expr.span.merge(right.span);
                expr = Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(expr),
                        right: Box::new(right),
                    },
                    span,
                };
            } else {
                break;
            }
        }
        expr
    }

    fn parse_range(&mut self) -> Expr {
        let mut expr = self.parse_add();
        while self.eat_punct(Punct::DotDot).is_some() {
            let right = self.parse_add();
            let span = expr.span.merge(right.span);
            expr = Expr {
                kind: ExprKind::Binary {
                    op: BinaryOp::Range,
                    left: Box::new(expr),
                    right: Box::new(right),
                },
                span,
            };
        }
        expr
    }

    fn parse_add(&mut self) -> Expr {
        let mut expr = self.parse_mul();
        loop {
            let op = if self.eat_punct(Punct::Plus).is_some() {
                Some(BinaryOp::Add)
            } else if self.eat_punct(Punct::Minus).is_some() {
                Some(BinaryOp::Sub)
            } else {
                None
            };
            if let Some(op) = op {
                let right = self.parse_mul();
                let span = expr.span.merge(right.span);
                expr = Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(expr),
                        right: Box::new(right),
                    },
                    span,
                };
            } else {
                break;
            }
        }
        expr
    }

    fn parse_mul(&mut self) -> Expr {
        let mut expr = self.parse_unary();
        loop {
            let op = if self.eat_punct(Punct::Star).is_some() {
                Some(BinaryOp::Mul)
            } else if self.eat_punct(Punct::Slash).is_some() {
                Some(BinaryOp::Div)
            } else if self.eat_punct(Punct::Percent).is_some() {
                Some(BinaryOp::Mod)
            } else {
                None
            };
            if let Some(op) = op {
                let right = self.parse_unary();
                let span = expr.span.merge(right.span);
                expr = Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(expr),
                        right: Box::new(right),
                    },
                    span,
                };
            } else {
                break;
            }
        }
        expr
    }

    fn parse_unary(&mut self) -> Expr {
        let start = self.peek_span();
        if self.eat_punct(Punct::Minus).is_some() {
            let expr = self.parse_unary();
            let span = start.merge(expr.span);
            return Expr {
                kind: ExprKind::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                },
                span,
            };
        }
        if self.eat_punct(Punct::Bang).is_some() {
            let expr = self.parse_unary();
            let span = start.merge(expr.span);
            return Expr {
                kind: ExprKind::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                },
                span,
            };
        }
        if self.eat_keyword(Keyword::Await).is_some() {
            let expr = self.parse_unary();
            let span = start.merge(expr.span);
            return Expr {
                kind: ExprKind::Await {
                    expr: Box::new(expr),
                },
                span,
            };
        }
        if self.eat_keyword(Keyword::Box).is_some() {
            let expr = self.parse_unary();
            let span = start.merge(expr.span);
            return Expr {
                kind: ExprKind::Box {
                    expr: Box::new(expr),
                },
                span,
            };
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut expr = self.parse_primary();
        loop {
            if self.eat_punct(Punct::LParen).is_some() {
                let args = self.parse_call_args();
                let end = self.expect_punct(Punct::RParen);
                let span = expr.span.merge(end);
                let is_struct_lit = matches!(&expr.kind, ExprKind::Ident(_))
                    && args.iter().any(|arg| arg.name.is_some());
                if is_struct_lit {
                    let name = match expr.kind {
                        ExprKind::Ident(id) => id,
                        _ => {
                            self.error_here("expected identifier for struct literal");
                            Ident {
                                name: "_".to_string(),
                                span: expr.span,
                            }
                        }
                    };
                    let fields = args
                        .into_iter()
                        .filter_map(|arg| {
                            arg.name.map(|n| StructField {
                                name: n,
                                value: arg.value,
                                span: arg.span,
                            })
                        })
                        .collect();
                    expr = Expr {
                        kind: ExprKind::StructLit { name, fields },
                        span,
                    };
                } else {
                    expr = Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    };
                }
                continue;
            }
            if self.eat_punct(Punct::Dot).is_some() {
                let name = self.expect_ident_or_body();
                let span = expr.span.merge(name.span);
                expr = Expr {
                    kind: ExprKind::Member {
                        base: Box::new(expr),
                        name,
                    },
                    span,
                };
                continue;
            }
            if self.eat_punct(Punct::Question).is_some() {
                self.expect_punct(Punct::Dot);
                let name = self.expect_ident_or_body();
                let span = expr.span.merge(name.span);
                expr = Expr {
                    kind: ExprKind::OptionalMember {
                        base: Box::new(expr),
                        name,
                    },
                    span,
                };
                continue;
            }
            if self.eat_punct(Punct::QuestionBang).is_some() {
                let mut error = None;
                let mut span = expr.span;
                if self.starts_expr() {
                    let err_expr = self.parse_expr();
                    span = span.merge(err_expr.span);
                    error = Some(Box::new(err_expr));
                }
                expr = Expr {
                    kind: ExprKind::BangChain {
                        expr: Box::new(expr),
                        error,
                    },
                    span,
                };
                continue;
            }
            break;
        }
        expr
    }

    fn parse_primary(&mut self) -> Expr {
        let start = self.peek_span();
        match self.peek_kind() {
            TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Bool(_)
            | TokenKind::String(_)
            | TokenKind::Null => {
                let lit = self.parse_literal();
                let span = start.merge(self.prev_span());
                Expr {
                    kind: ExprKind::Literal(lit),
                    span,
                }
            }
            TokenKind::InterpString(_) => {
                let tok = self.bump();
                let span = tok.span;
                let parts = match tok.kind {
                    TokenKind::InterpString(segments) => self.parse_interp_segments(segments, span),
                    _ => Vec::new(),
                };
                Expr {
                    kind: ExprKind::InterpString(parts),
                    span,
                }
            }
            TokenKind::Ident(_) | TokenKind::Keyword(Keyword::Body) => {
                let ident = self.expect_ident_or_body();
                let span = ident.span;
                Expr {
                    kind: ExprKind::Ident(ident),
                    span,
                }
            }
            TokenKind::Keyword(Keyword::Spawn) => {
                let start = self.peek_span();
                self.bump();
                self.expect_punct(Punct::Colon);
                let block = self.parse_block();
                let span = start.merge(block.span);
                Expr {
                    kind: ExprKind::Spawn { block },
                    span,
                }
            }
            TokenKind::Punct(Punct::LParen) => {
                self.bump();
                let mut expr = self.parse_expr();
                let end = self.expect_punct(Punct::RParen);
                expr.span = expr.span.merge(end);
                expr
            }
            TokenKind::Punct(Punct::LBracket) => {
                self.bump();
                let mut items = Vec::new();
                if !self.at_punct(Punct::RBracket) {
                    loop {
                        items.push(self.parse_expr());
                        if self.eat_punct(Punct::Comma).is_none() {
                            break;
                        }
                    }
                }
                let end = self.expect_punct(Punct::RBracket);
                Expr {
                    kind: ExprKind::ListLit(items),
                    span: start.merge(end),
                }
            }
            TokenKind::Punct(Punct::LBrace) => {
                self.bump();
                let mut pairs = Vec::new();
                if !self.at_punct(Punct::RBrace) {
                    loop {
                        let key = self.parse_expr();
                        self.expect_punct(Punct::Colon);
                        let value = self.parse_expr();
                        pairs.push((key, value));
                        if self.eat_punct(Punct::Comma).is_none() {
                            break;
                        }
                    }
                }
                let end = self.expect_punct(Punct::RBrace);
                Expr {
                    kind: ExprKind::MapLit(pairs),
                    span: start.merge(end),
                }
            }
            _ => {
                self.error_here("expected expression");
                self.bump();
                Expr {
                    kind: ExprKind::Literal(Literal::Null),
                    span: start,
                }
            }
        }
    }

    fn parse_interp_segments(
        &mut self,
        segments: Vec<InterpSegment>,
        _span: Span,
    ) -> Vec<InterpPart> {
        let mut parts = Vec::new();
        for segment in segments {
            match segment {
                InterpSegment::Text(text) => {
                    if !text.is_empty() {
                        parts.push(InterpPart::Text(text));
                    }
                }
                InterpSegment::Expr { src, offset } => {
                    let expr_span = Span::new(offset, offset + src.len());
                    if src.trim().is_empty() {
                        self.diags.error(expr_span, "empty interpolation expression");
                        parts.push(InterpPart::Expr(Expr {
                            kind: ExprKind::Literal(Literal::Null),
                            span: expr_span,
                        }));
                        continue;
                    }
                    let expr = self.parse_interpolated_expr(&src, offset, expr_span);
                    parts.push(InterpPart::Expr(expr));
                }
            }
        }
        if parts.is_empty() {
            parts.push(InterpPart::Text(String::new()));
        }
        parts
    }

    fn parse_interpolated_expr(&mut self, src: &str, offset: usize, span: Span) -> Expr {
        let mut lex_diags = Diagnostics::default();
        let mut tokens = lexer::lex(src, &mut lex_diags);
        for token in &mut tokens {
            token.span.start += offset;
            token.span.end += offset;
        }
        let mut parse_diags = Diagnostics::default();
        let mut parser = Parser::new(&tokens, &mut parse_diags);
        let expr = parser.parse_expr();
        parser.consume_newlines();
        if !parser.at_eof() {
            parse_diags.error(span, "unexpected tokens in interpolation");
        }
        let mut lex_items = lex_diags.into_vec();
        for diag in &mut lex_items {
            diag.span.start += offset;
            diag.span.end += offset;
        }
        self.diags.extend(lex_items);
        self.diags.extend(parse_diags.into_vec());
        expr
    }

    fn parse_call_args(&mut self) -> Vec<CallArg> {
        let mut args = Vec::new();
        if self.at_punct(Punct::RParen) {
            return args;
        }
        loop {
            let start = self.peek_span();
            let (name, value) = if self.is_named_arg() {
                let name = self.expect_ident();
                self.expect_punct(Punct::Assign);
                let value = self.parse_expr();
                (Some(name), value)
            } else {
                (None, self.parse_expr())
            };
            let end = self.prev_span();
            args.push(CallArg {
                name,
                value,
                span: start.merge(end),
            });
            if self.eat_punct(Punct::Comma).is_none() {
                break;
            }
        }
        args
    }

    fn parse_literal(&mut self) -> Literal {
        match self.bump().kind {
            TokenKind::Int(v) => Literal::Int(v),
            TokenKind::Float(v) => Literal::Float(v),
            TokenKind::Bool(v) => Literal::Bool(v),
            TokenKind::String(v) => Literal::String(v),
            TokenKind::Null => Literal::Null,
            _ => {
                self.error_here("expected literal");
                Literal::Null
            }
        }
    }

    fn parse_type_name(&mut self) -> Ident {
        let mut base = self.expect_ident();
        let mut full_name = base.name.clone();
        let mut span = base.span;
        while self.eat_punct(Punct::Dot).is_some() {
            let part = self.expect_ident();
            if full_name.is_empty() {
                full_name = part.name.clone();
            } else {
                full_name.push('.');
                full_name.push_str(&part.name);
            }
            span = span.merge(part.span);
        }
        base.name = full_name;
        base.span = span;
        base
    }

    fn parse_type_ref(&mut self) -> TypeRef {
        let start = self.peek_span();
        let base = self.parse_type_name();
        let mut kind = if self.eat_punct(Punct::Lt).is_some() {
            let mut args = Vec::new();
            if !self.at_punct(Punct::Gt) {
                loop {
                    args.push(self.parse_type_ref());
                    if self.eat_punct(Punct::Comma).is_none() {
                        break;
                    }
                }
            }
            TypeRefKind::Generic { base, args }
        } else if self.eat_punct(Punct::LParen).is_some() {
            let mut args = Vec::new();
            if !self.at_punct(Punct::RParen) {
                loop {
                    args.push(self.parse_expr());
                    if self.eat_punct(Punct::Comma).is_none() {
                        break;
                    }
                }
            }
            self.expect_punct(Punct::RParen);
            TypeRefKind::Refined { base, args }
        } else {
            TypeRefKind::Simple(base)
        };
        let mut span = start.merge(self.prev_span());
        loop {
            if self.eat_punct(Punct::Question).is_some() {
                let ty = TypeRef { kind, span };
                span = span.merge(self.prev_span());
                kind = TypeRefKind::Optional(Box::new(ty));
                continue;
            }
            if self.eat_punct(Punct::Bang).is_some() {
                let ok = TypeRef { kind, span };
                let err = if self.starts_type_ref() {
                    Some(Box::new(self.parse_type_ref()))
                } else {
                    None
                };
                span = ok.span.merge(self.prev_span());
                kind = TypeRefKind::Result {
                    ok: Box::new(ok),
                    err,
                };
                continue;
            }
            break;
        }
        TypeRef { kind, span }
    }

    fn take_doc_comments(&mut self) -> Option<String> {
        let mut docs = Vec::new();
        loop {
            self.consume_newlines();
            match self.peek_kind() {
                TokenKind::DocComment(text) => {
                    let text = text.clone();
                    self.bump();
                    docs.push(text);
                }
                _ => break,
            }
        }
        if docs.is_empty() {
            None
        } else {
            Some(docs.join("\n"))
        }
    }

    fn is_named_arg(&self) -> bool {
        match (self.peek_kind(), self.peek_kind_n(1)) {
            (TokenKind::Ident(_), TokenKind::Punct(Punct::Assign)) => true,
            _ => false,
        }
    }

    fn is_named_pattern(&self) -> bool {
        self.is_named_arg()
    }

    fn starts_expr(&self) -> bool {
        match self.peek_kind() {
            TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::String(_)
            | TokenKind::InterpString(_)
            | TokenKind::Bool(_)
            | TokenKind::Null
            | TokenKind::Keyword(Keyword::Spawn)
            | TokenKind::Keyword(Keyword::Await)
            | TokenKind::Keyword(Keyword::Box)
            | TokenKind::Punct(Punct::LParen)
            | TokenKind::Punct(Punct::LBracket)
            | TokenKind::Punct(Punct::LBrace) => true,
            _ => false,
        }
    }

    fn starts_type_ref(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Ident(_))
    }

    fn sync_to_next_item(&mut self) {
        while !self.at_eof() {
            if self.at_newline() {
                self.bump();
                if self.peek_kind().is_keyword_item_start() {
                    break;
                }
            } else {
                self.bump();
            }
        }
    }

    fn expect_indent(&mut self) -> Span {
        match self.bump().kind {
            TokenKind::Indent => self.prev_span(),
            _ => {
                self.error_here("expected indent");
                self.prev_span()
            }
        }
    }

    fn expect_dedent(&mut self) -> Span {
        match self.bump().kind {
            TokenKind::Dedent => self.prev_span(),
            _ => {
                self.error_here("expected dedent");
                self.prev_span()
            }
        }
    }

    fn expect_newline(&mut self) {
        match self.bump().kind {
            TokenKind::Newline => {}
            _ => self.error_here("expected newline"),
        }
    }

    fn expect_ident(&mut self) -> Ident {
        match self.bump().kind {
            TokenKind::Ident(name) => Ident {
                name,
                span: self.prev_span(),
            },
            _ => {
                self.error_here("expected identifier");
                Ident {
                    name: "_".to_string(),
                    span: self.prev_span(),
                }
            }
        }
    }

    fn expect_ident_or_body(&mut self) -> Ident {
        match self.bump().kind {
            TokenKind::Ident(name) => Ident {
                name,
                span: self.prev_span(),
            },
            TokenKind::Keyword(Keyword::Body) => Ident {
                name: "body".to_string(),
                span: self.prev_span(),
            },
            _ => {
                self.error_here("expected identifier");
                Ident {
                    name: "_".to_string(),
                    span: self.prev_span(),
                }
            }
        }
    }

    fn expect_string_lit(&mut self) -> StringLit {
        match self.bump().kind {
            TokenKind::String(value) => StringLit {
                value,
                span: self.prev_span(),
            },
            _ => {
                self.error_here("expected string literal");
                StringLit {
                    value: String::new(),
                    span: self.prev_span(),
                }
            }
        }
    }

    fn expect_keyword(&mut self, kw: Keyword) {
        if self.eat_keyword(kw).is_none() {
            self.error_here("expected keyword");
        }
    }

    fn expect_punct(&mut self, punct: Punct) -> Span {
        match self.bump().kind {
            TokenKind::Punct(p) if p == punct => self.prev_span(),
            _ => {
                self.error_here("expected punctuation");
                self.prev_span()
            }
        }
    }

    fn eat_keyword(&mut self, kw: Keyword) -> Option<Token> {
        if matches!(self.peek_kind(), TokenKind::Keyword(k) if *k == kw) {
            Some(self.bump())
        } else {
            None
        }
    }

    fn eat_punct(&mut self, punct: Punct) -> Option<Token> {
        if matches!(self.peek_kind(), TokenKind::Punct(p) if *p == punct) {
            Some(self.bump())
        } else {
            None
        }
    }

    fn at_punct(&self, punct: Punct) -> bool {
        matches!(self.peek_kind(), TokenKind::Punct(p) if *p == punct)
    }

    fn at_newline(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Newline)
    }

    fn at_dedent(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Dedent)
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    fn consume_newlines(&mut self) {
        while self.at_newline() {
            self.bump();
        }
    }

    fn error_here(&mut self, message: &str) {
        let span = self.peek_span();
        self.diags.error(span, message);
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or_else(|| {
            self.tokens.last().expect("token stream should end with Eof")
        })
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn peek_kind_n(&self, n: usize) -> &TokenKind {
        self.tokens
            .get(self.pos + n)
            .map(|t| &t.kind)
            .unwrap_or_else(|| &self.tokens.last().unwrap().kind)
    }

    fn bump(&mut self) -> Token {
        let tok = self.peek().clone();
        if !self.at_eof() {
            self.pos += 1;
        }
        tok
    }

    fn peek_span(&self) -> Span {
        self.peek().span
    }

    fn prev_span(&self) -> Span {
        if self.pos == 0 {
            self.peek().span
        } else {
            self.tokens[self.pos - 1].span
        }
    }
}

trait ItemStart {
    fn is_keyword_item_start(&self) -> bool;
}

impl ItemStart for TokenKind {
    fn is_keyword_item_start(&self) -> bool {
        matches!(
            self,
            TokenKind::Keyword(Keyword::Import)
                | TokenKind::Keyword(Keyword::Type)
                | TokenKind::Keyword(Keyword::Enum)
                | TokenKind::Keyword(Keyword::Fn)
                | TokenKind::Keyword(Keyword::Service)
                | TokenKind::Keyword(Keyword::Config)
                | TokenKind::Keyword(Keyword::App)
                | TokenKind::Keyword(Keyword::Migration)
                | TokenKind::Keyword(Keyword::Test)
        )
    }
}
