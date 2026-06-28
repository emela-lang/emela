use crate::ast::{
    BinaryOp, Block, BlockItem, Capability, EnumDecl, EnumVariant, Expr, Function, FunctionParam,
    FunctionType, ImportDecl, ImportOrigin, MatchArm, Pattern, PrimType, Program, StructDecl,
    StructField, TopLevelItem, Type,
};
use crate::error::{Diagnostic, Error, Result, SourceFile, Span};
use crate::lexer::{lex_with_file, Token, TokenKind};

pub(crate) fn parse_program(label: &str, source: &str) -> Result<Program> {
    let file = SourceFile::new(label, source.to_string());
    let tokens = lex_with_file(source, file)?;
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

pub(crate) struct Parser {
    tokens: Vec<Token>,
    current: usize,
}

impl Parser {
    pub(crate) fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, current: 0 }
    }

    pub(crate) fn parse_program(&mut self) -> Result<Program> {
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::Eof) {
            if self.at(&TokenKind::Import) {
                items.push(TopLevelItem::Import(self.parse_import_decl()?));
            } else if self.at(&TokenKind::Struct) {
                items.push(TopLevelItem::Struct(self.parse_struct_decl()?));
            } else if self.at(&TokenKind::Enum) {
                items.push(TopLevelItem::Enum(self.parse_enum_decl()?));
            } else {
                let attributes = self.parse_attributes()?;
                items.push(TopLevelItem::Function(self.parse_function(attributes)?));
            }
            self.skip_newlines();
        }
        Ok(Program { items })
    }

    fn parse_import_decl(&mut self) -> Result<ImportDecl> {
        let start = self.peek().span.clone();
        self.expect(&TokenKind::Import)?;
        let mut path = Vec::new();
        path.push(self.expect_ident()?);
        while self.eat(&TokenKind::Dot) {
            path.push(self.expect_ident()?);
        }
        let name = path
            .pop()
            .ok_or_else(|| Error::new("import path must not be empty"))?;
        let name = if self.eat(&TokenKind::Bang) {
            format!("{name}!")
        } else {
            name
        };
        if path.is_empty() {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid import")
                    .label(
                        start.merge(self.previous_span()),
                        "import path must include a package and item.",
                    )
                    .help("Write imports as `import package.module.item`."),
            ));
        }
        let span = start.merge(self.previous_span());
        Ok(ImportDecl {
            path,
            name,
            origin: ImportOrigin::User,
            span,
        })
    }

    fn parse_struct_decl(&mut self) -> Result<StructDecl> {
        self.expect(&TokenKind::Struct)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        let type_params = self.parse_type_parameter_list()?;
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();
        let field_name = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let (ty, ty_span) = self.parse_type_with_span()?;
        self.skip_newlines();
        self.expect(&TokenKind::RBrace)?;
        Ok(StructDecl {
            name,
            name_span,
            type_params,
            field: StructField {
                name: field_name,
                ty,
                ty_span,
            },
        })
    }

    fn parse_enum_decl(&mut self) -> Result<EnumDecl> {
        self.expect(&TokenKind::Enum)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        let type_params = self.parse_type_parameter_list()?;
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();
        let mut variants = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unterminated enum")
                        .label(self.peek().span.clone(), "unterminated enum declaration.")
                        .help("Add a closing `}` for this enum declaration."),
                ));
            }
            let variant_name_span = self.peek().span.clone();
            let variant_name = self.expect_ident()?;
            let (payload, payload_span) = if self.eat(&TokenKind::LParen) {
                let (ty, ty_span) = self.parse_type_with_span()?;
                self.expect(&TokenKind::RParen)?;
                (Some(ty), Some(ty_span))
            } else {
                (None, None)
            };
            variants.push(EnumVariant {
                name: variant_name,
                name_span: variant_name_span,
                payload,
                payload_span,
            });
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(EnumDecl {
            name,
            name_span,
            type_params,
            variants,
        })
    }

    fn parse_type_parameter_list(&mut self) -> Result<Vec<String>> {
        if !self.eat(&TokenKind::Lt) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        params.push(self.expect_ident()?);
        while self.eat(&TokenKind::Comma) {
            params.push(self.expect_ident()?);
        }
        self.expect(&TokenKind::Gt)?;
        Ok(params)
    }

    fn parse_attributes(&mut self) -> Result<FunctionAttributes> {
        let mut attributes = FunctionAttributes::default();
        loop {
            self.skip_newlines();
            if !self.at(&TokenKind::Hash) {
                return Ok(attributes);
            }
            self.bump();
            self.expect(&TokenKind::LBracket)?;
            let attribute_name = self.expect_ident()?;
            match attribute_name.as_str() {
                "requires" => {
                    if attributes.requires.is_some() {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Duplicate attribute")
                                .label(
                                    self.previous_span().clone(),
                                    "duplicate #[requires(...)] attribute.",
                                )
                                .help("Keep a single `#[requires(...)]` attribute and merge the capabilities."),
                        ));
                    }
                    self.expect(&TokenKind::LParen)?;
                    let mut capabilities = Vec::new();
                    if !self.at(&TokenKind::RParen) {
                        capabilities.push(self.expect_capability()?);
                        while self.eat(&TokenKind::Comma) {
                            capabilities.push(self.expect_capability()?);
                        }
                    }
                    self.expect(&TokenKind::RParen)?;
                    attributes.requires = Some(capabilities);
                }
                _ => {
                    return Err(Error::diagnostic(
                        Diagnostic::new("Unsupported attribute")
                            .label(
                                self.previous_span().clone(),
                                format!("unsupported attribute `#[{attribute_name}]`."),
                            )
                            .help("Only `#[requires(...)]` is supported here."),
                    ));
                }
            }
            self.expect(&TokenKind::RBracket)?;
        }
    }

    fn parse_function(&mut self, attributes: FunctionAttributes) -> Result<Function> {
        self.expect(&TokenKind::Fn)?;
        let name_span = self.peek().span.clone();
        let name = self.parse_function_name()?;
        let type_params = self.parse_type_parameter_list()?;
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            params.push(self.parse_function_param()?);
            while self.eat(&TokenKind::Comma) {
                params.push(self.parse_function_param()?);
            }
        }
        self.expect(&TokenKind::RParen)?;
        let (return_annotation, return_annotation_span) = if self.eat(&TokenKind::Arrow) {
            let (ty, span) = self.parse_type_with_span()?;
            (Some(ty), Some(span))
        } else {
            (None, None)
        };
        let body = self.parse_block()?;
        Ok(Function {
            name,
            name_span,
            type_params,
            params,
            return_annotation,
            return_annotation_span,
            requires: attributes.requires,
            body,
        })
    }

    fn parse_function_param(&mut self) -> Result<FunctionParam> {
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        let (ty, ty_span) = if self.eat(&TokenKind::Colon) {
            let (ty, span) = self.parse_type_with_span()?;
            (Some(ty), Some(span))
        } else {
            (None, None)
        };
        Ok(FunctionParam {
            name,
            name_span,
            ty,
            ty_span,
        })
    }

    fn parse_function_name(&mut self) -> Result<String> {
        let mut name = self.expect_ident()?;
        if self.eat(&TokenKind::Bang) {
            name.push('!');
        }
        Ok(name)
    }

    fn parse_block(&mut self) -> Result<Block> {
        self.expect(&TokenKind::LBrace)?;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unterminated block")
                        .label(self.peek().span.clone(), "unterminated block.")
                        .help("Add a closing `}` for this block."),
                ));
            }

            if self.starts_binding() {
                let name = self.expect_ident()?;
                let (ty, ty_span) = if self.eat(&TokenKind::Colon) {
                    let (ty, span) = self.parse_type_with_span()?;
                    (Some(ty), Some(span))
                } else {
                    (None, None)
                };
                self.expect(&TokenKind::Eq)?;
                let expr = self.parse_expr()?;
                let span = expr.span().clone();
                items.push(BlockItem::Binding {
                    name,
                    ty,
                    ty_span,
                    expr,
                    span,
                });
            } else {
                items.push(BlockItem::Expr(self.parse_expr()?));
            }
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Block { items })
    }

    fn starts_binding(&self) -> bool {
        if !matches!(self.peek().kind, TokenKind::Ident(_)) {
            return false;
        }
        matches!(
            self.peek_n(1).map(|token| &token.kind),
            Some(TokenKind::Eq | TokenKind::Colon)
        )
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_pipeline()
    }

    fn parse_pipeline(&mut self) -> Result<Expr> {
        let mut expr = self.parse_equality()?;
        loop {
            let checkpoint = self.current;
            self.skip_newlines();
            if !self.eat(&TokenKind::Pipe) {
                self.current = checkpoint;
                break;
            }

            let name = self.parse_function_name()?;
            let type_args = self.parse_type_argument_list_for_call()?;
            if !self.eat(&TokenKind::LParen) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Invalid pipeline stage")
                        .label(
                            self.peek().span.clone(),
                            "pipeline stage must be an explicit function call.",
                        )
                        .help("Write the stage as `name(...)`; the piped value is inserted as the first argument."),
                ));
            }
            let span = expr.span().clone();
            let mut args = vec![expr];
            args.extend(self.parse_argument_list()?);
            self.expect(&TokenKind::RParen)?;
            expr = Expr::Call {
                name,
                type_args,
                args,
                span,
            };
        }
        Ok(expr)
    }

    fn parse_equality(&mut self) -> Result<Expr> {
        let mut expr = self.parse_sum()?;
        loop {
            let op = if self.eat(&TokenKind::EqEq) {
                BinaryOp::Eq
            } else if self.eat(&TokenKind::Lt) {
                BinaryOp::Lt
            } else {
                break;
            };
            let right = self.parse_sum()?;
            expr = Expr::Binary {
                op,
                span: expr.span().merge(right.span()),
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_sum(&mut self) -> Result<Expr> {
        let mut expr = self.parse_product()?;
        loop {
            let op = if self.eat(&TokenKind::Plus) {
                BinaryOp::Add
            } else if self.eat(&TokenKind::Minus) {
                BinaryOp::Sub
            } else {
                break;
            };
            let right = self.parse_product()?;
            expr = Expr::Binary {
                op,
                span: expr.span().merge(right.span()),
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_product(&mut self) -> Result<Expr> {
        let mut expr = self.parse_postfix()?;
        while self.eat(&TokenKind::Star) {
            let right = self.parse_postfix()?;
            expr = Expr::Binary {
                op: BinaryOp::Mul,
                span: expr.span().merge(right.span()),
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_postfix(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.eat(&TokenKind::Dot) {
                let name = self.expect_ident()?;
                if self.eat(&TokenKind::LParen) {
                    let args = self.parse_argument_list()?;
                    self.expect(&TokenKind::RParen)?;
                    expr = Expr::MethodCall {
                        span: expr.span().clone(),
                        receiver: Box::new(expr),
                        name,
                        args,
                    };
                } else {
                    expr = Expr::FieldAccess {
                        span: expr.span().clone(),
                        receiver: Box::new(expr),
                        field: name,
                    };
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.peek().kind.clone() {
            TokenKind::Int(value) => {
                let span = self.peek().span.clone();
                self.bump();
                Ok(Expr::Int(value, span))
            }
            TokenKind::String(value) => {
                let span = self.peek().span.clone();
                self.bump();
                Ok(Expr::String(value, span))
            }
            TokenKind::True => {
                let span = self.peek().span.clone();
                self.bump();
                Ok(Expr::Bool(true, span))
            }
            TokenKind::False => {
                let span = self.peek().span.clone();
                self.bump();
                Ok(Expr::Bool(false, span))
            }
            TokenKind::Fn => self.parse_lambda_expr(),
            TokenKind::Ident(_) => {
                let span = self.peek().span.clone();
                let name = self.parse_function_name()?;
                if self.at(&TokenKind::LBrace) && starts_with_uppercase(&name) {
                    self.bump();
                    let field = self.expect_ident()?;
                    self.expect(&TokenKind::Colon)?;
                    let value = self.parse_expr()?;
                    self.expect(&TokenKind::RBrace)?;
                    return Ok(Expr::StructLiteral {
                        name,
                        field,
                        value: Box::new(value),
                        span,
                    });
                }
                if self.eat(&TokenKind::LParen) {
                    let args = self.parse_argument_list()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr::Call {
                        name,
                        type_args: Vec::new(),
                        args,
                        span,
                    })
                } else {
                    let checkpoint = self.current;
                    let type_args = if self.at(&TokenKind::Lt) {
                        match self.parse_type_argument_list_for_call() {
                            Ok(type_args) if self.at(&TokenKind::LParen) => type_args,
                            _ => {
                                self.current = checkpoint;
                                Vec::new()
                            }
                        }
                    } else {
                        Vec::new()
                    };
                    if self.eat(&TokenKind::LParen) {
                        let args = self.parse_argument_list()?;
                        self.expect(&TokenKind::RParen)?;
                        Ok(Expr::Call {
                            name,
                            type_args,
                            args,
                            span,
                        })
                    } else {
                        Ok(Expr::Var(name, span))
                    }
                }
            }
            TokenKind::Match => {
                let span = self.peek().span.clone();
                self.bump();
                let scrutinee = self.parse_expr()?;
                self.expect(&TokenKind::LBrace)?;
                let mut arms = Vec::new();
                self.skip_newlines();
                while !self.at(&TokenKind::RBrace) {
                    if self.at(&TokenKind::Eof) {
                        return Err(Error::diagnostic(
                            Diagnostic::new("Unterminated match")
                                .label(self.peek().span.clone(), "unterminated match expression.")
                                .help("Add a closing `}` for this match expression."),
                        ));
                    }
                    let pattern = self.parse_pattern()?;
                    self.expect(&TokenKind::Arrow)?;
                    let expr = self.parse_expr()?;
                    arms.push(MatchArm { pattern, expr });
                    self.skip_newlines();
                }
                self.expect(&TokenKind::RBrace)?;
                Ok(Expr::Match {
                    scrutinee: Box::new(scrutinee),
                    arms,
                    span,
                })
            }
            TokenKind::LBrace => {
                let span = self.peek().span.clone();
                Ok(Expr::Block(self.parse_block()?, span))
            }
            TokenKind::LParen => {
                let span = self.peek().span.clone();
                self.bump();
                if self.eat(&TokenKind::RParen) {
                    Ok(Expr::Unit(span))
                } else {
                    let expr = self.parse_expr()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(expr)
                }
            }
            _ => Err(Error::diagnostic(
                Diagnostic::new("Expected an expression")
                    .label(
                        self.peek().span.clone(),
                        "I expected to find an expression here.",
                    )
                    .help("Add a value, a function call, a block, or `()` for Unit."),
            )),
        }
    }

    fn parse_lambda_expr(&mut self) -> Result<Expr> {
        let start = self.peek().span.clone();
        self.expect(&TokenKind::Fn)?;
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            params.push(self.parse_function_param()?);
            while self.eat(&TokenKind::Comma) {
                params.push(self.parse_function_param()?);
            }
        }
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let body = self.parse_expr()?;
        let span = start.merge(body.span());
        Ok(Expr::Lambda {
            params,
            body: Box::new(body),
            span,
        })
    }

    fn parse_argument_list(&mut self) -> Result<Vec<Expr>> {
        let mut args = Vec::new();
        if !self.at(&TokenKind::RParen) {
            args.push(self.parse_expr()?);
            while self.eat(&TokenKind::Comma) {
                args.push(self.parse_expr()?);
            }
        }
        Ok(args)
    }

    fn parse_type_argument_list_for_call(&mut self) -> Result<Vec<Type>> {
        if !self.eat(&TokenKind::Lt) {
            return Ok(Vec::new());
        }
        let mut args = Vec::new();
        args.push(self.parse_type()?);
        while self.eat(&TokenKind::Comma) {
            args.push(self.parse_type()?);
        }
        self.expect(&TokenKind::Gt)?;
        Ok(args)
    }

    fn parse_type(&mut self) -> Result<Type> {
        Ok(self.parse_type_with_span()?.0)
    }

    fn parse_type_with_span(&mut self) -> Result<(Type, Span)> {
        let start = self.peek().span.clone();
        if self.at(&TokenKind::Fn) {
            let ty = self.parse_function_type()?;
            let span = start.merge(self.previous_span());
            return Ok((ty, span));
        }
        let name = self.expect_ident()?;
        let base = match name.as_str() {
            "I32" | "i32" => Type::Prim(PrimType::I32),
            "Bool" | "bool" => Type::Prim(PrimType::Bool),
            "String" | "string" => Type::Prim(PrimType::String),
            "Unit" | "unit" => Type::Prim(PrimType::Unit),
            _ => Type::Named(name),
        };
        if !self.eat(&TokenKind::Lt) {
            let span = start.merge(self.previous_span());
            return Ok((base, span));
        }
        let Type::Named(name) = base else {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid type arguments")
                    .label(
                        start.merge(self.previous_span()),
                        "only named types can take type arguments.",
                    )
                    .help("Remove the `<...>` from primitive and function types."),
            ));
        };
        let mut args = Vec::new();
        args.push(self.parse_type()?);
        while self.eat(&TokenKind::Comma) {
            args.push(self.parse_type()?);
        }
        self.expect(&TokenKind::Gt)?;
        let span = start.merge(self.previous_span());
        Ok((Type::Apply { name, args }, span))
    }

    fn parse_function_type(&mut self) -> Result<Type> {
        self.expect(&TokenKind::Fn)?;
        let effectful = self.eat(&TokenKind::Bang);
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            params.push(self.parse_type()?);
            while self.eat(&TokenKind::Comma) {
                params.push(self.parse_type()?);
            }
        }
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        Ok(Type::Function(FunctionType {
            params,
            ret: Box::new(ret),
            effectful,
        }))
    }

    fn parse_pattern(&mut self) -> Result<Pattern> {
        match self.peek().kind.clone() {
            TokenKind::Int(value) => {
                self.bump();
                Ok(Pattern::Int(value))
            }
            TokenKind::True => {
                self.bump();
                Ok(Pattern::Bool(true))
            }
            TokenKind::False => {
                self.bump();
                Ok(Pattern::Bool(false))
            }
            TokenKind::LParen => {
                self.bump();
                self.expect(&TokenKind::RParen)?;
                Ok(Pattern::Unit)
            }
            TokenKind::Ident(name) if name == "_" => {
                self.bump();
                Ok(Pattern::Wildcard)
            }
            TokenKind::Ident(_) => {
                let name = self.expect_ident()?;
                if self.eat(&TokenKind::LParen) {
                    let payload = self.parse_pattern()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(Pattern::Variant {
                        name,
                        payload: Some(Box::new(payload)),
                    })
                } else if name
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_uppercase())
                {
                    Ok(Pattern::Variant {
                        name,
                        payload: None,
                    })
                } else {
                    Ok(Pattern::Var(name))
                }
            }
            _ => Err(Error::diagnostic(
                Diagnostic::new("Expected a pattern")
                    .label(
                        self.peek().span.clone(),
                        "I expected to find a match pattern here.",
                    )
                    .help("Use a literal, `_`, a binding name, or an enum variant pattern."),
            )),
        }
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.peek().kind.clone() {
            TokenKind::Ident(name) => {
                self.bump();
                Ok(name)
            }
            _ => Err(Error::diagnostic(
                Diagnostic::new("Expected a name")
                    .label(self.peek().span.clone(), "I expected a name here.")
                    .help("Names start with a letter or `_`, followed by letters, digits, or `_`."),
            )),
        }
    }

    fn expect_capability(&mut self) -> Result<Capability> {
        let span = self.peek().span.clone();
        let name = self.expect_ident()?;
        Capability::parse(&name).ok_or_else(|| {
            Error::diagnostic(
                Diagnostic::new("Unknown capability")
                    .label(span, format!("unknown capability `{name}`."))
                    .help("Use one of the capabilities supported by the platform."),
            )
        })
    }

    fn skip_newlines(&mut self) {
        while self.at(&TokenKind::Newline) {
            self.bump();
        }
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<()> {
        if self.eat(kind) {
            Ok(())
        } else {
            Err(Error::diagnostic(
                Diagnostic::new("Unexpected syntax")
                    .label(
                        self.peek().span.clone(),
                        format!("I expected `{}` here.", token_name(kind)),
                    )
                    .help("Check the surrounding syntax and add the missing token."),
            ))
        }
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek().kind == *kind
    }

    fn bump(&mut self) {
        self.current += 1;
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn peek_n(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.current + n)
    }

    fn previous_span(&self) -> &Span {
        &self.tokens[self.current.saturating_sub(1)].span
    }
}

#[derive(Default)]
struct FunctionAttributes {
    requires: Option<Vec<Capability>>,
}

fn starts_with_uppercase(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn token_name(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::Fn => "fn",
        TokenKind::Import => "import",
        TokenKind::Struct => "struct",
        TokenKind::Enum => "enum",
        TokenKind::Match => "match",
        TokenKind::True => "true",
        TokenKind::False => "false",
        TokenKind::Ident(_) => "name",
        TokenKind::Int(_) => "integer",
        TokenKind::String(_) => "string",
        TokenKind::LParen => "(",
        TokenKind::RParen => ")",
        TokenKind::LBrace => "{",
        TokenKind::RBrace => "}",
        TokenKind::LBracket => "[",
        TokenKind::RBracket => "]",
        TokenKind::Comma => ",",
        TokenKind::Dot => ".",
        TokenKind::Colon => ":",
        TokenKind::Hash => "#",
        TokenKind::Bang => "!",
        TokenKind::Eq => "=",
        TokenKind::EqEq => "==",
        TokenKind::Arrow => "->",
        TokenKind::Pipe => "|>",
        TokenKind::Lt => "<",
        TokenKind::Gt => ">",
        TokenKind::Plus => "+",
        TokenKind::Minus => "-",
        TokenKind::Star => "*",
        TokenKind::Newline => "newline",
        TokenKind::Eof => "end of file",
    }
}
