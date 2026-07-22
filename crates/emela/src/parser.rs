use crate::ast::{
    BinaryOp, Block, BlockItem, Bound, EffectDecl, EffectRow, EnumDecl, EnumVariant, Expr, Extern,
    FieldBinding, Function, FunctionType, ImplDecl, Import, MatchArm, Param, Pattern, Program,
    RecordDecl, RecordFieldDef, TraitDecl, TraitMethodSig, Type,
};
use crate::error::{Diagnostic, Error, Result, Span};
use crate::lexer::{Token, TokenKind, lex};

/// Parses `source`, collecting every error instead of stopping at the first
/// (spec 0033). A failed top-level declaration is skipped (see
/// `recover_to_top_level`), so the returned `Program` holds every declaration
/// that did parse. An empty error list means the parse is complete.
pub(crate) fn parse_program(label: &str, source: &str) -> (Program, Vec<Error>) {
    let (tokens, mut errors) = lex(label, source);
    let mut parser = Parser {
        tokens,
        current: 0,
        type_params: Vec::new(),
    };
    let program = parser.parse_program(&mut errors);
    (program, errors)
}

struct Parser {
    tokens: Vec<Token>,
    current: usize,
    /// The type parameters declared by the function currently being parsed
    /// (spec 0014). While non-empty, `parse_type` resolves a bare name in this
    /// set to `Type::Var` instead of a named enum. Functions never nest, so a
    /// single set (cleared per function) is enough.
    type_params: Vec<String>,
}

impl Parser {
    fn parse_program(&mut self, errors: &mut Vec<Error>) -> Program {
        let mut module = None;
        let mut imports = Vec::new();
        let mut functions = Vec::new();
        let mut externs = Vec::new();
        let mut enums = Vec::new();
        let mut records = Vec::new();
        let mut traits = Vec::new();
        let mut impls = Vec::new();
        let mut effects = Vec::new();
        self.skip_newlines();
        if self.eat(&TokenKind::Module) {
            match self.parse_path_name() {
                Ok(name) => module = Some(name),
                Err(error) => {
                    errors.push(error);
                    self.recover_to_top_level();
                }
            }
            self.skip_newlines();
        }
        while !self.at(&TokenKind::Eof) {
            // Attributes (spec 0039) precede the declaration they modify; they
            // are collected first and validated once the declaration's kind is
            // known (`apply_attributes`).
            let attrs = self.parse_attributes(errors);
            let result = if self.at(&TokenKind::Import) {
                self.parse_import().map(|import| {
                    imports.push(import);
                    DeclKind::Import
                })
            } else if self.at(&TokenKind::Extern) {
                self.parse_extern().map(|declaration| {
                    externs.push(declaration);
                    DeclKind::Extern
                })
            } else if self.at(&TokenKind::Intrinsic) {
                self.parse_intrinsic().map(|declaration| {
                    externs.push(declaration);
                    DeclKind::Extern
                })
            } else if self.at(&TokenKind::Enum) {
                self.parse_enum().map(|decl| {
                    enums.push(decl);
                    DeclKind::Enum
                })
            } else if self.at(&TokenKind::Record) {
                self.parse_record().map(|decl| {
                    records.push(decl);
                    DeclKind::Record
                })
            } else if self.at(&TokenKind::Trait) {
                self.parse_trait().map(|decl| {
                    traits.push(decl);
                    DeclKind::Trait
                })
            } else if self.at(&TokenKind::Impl) {
                self.parse_impl().map(|decl| {
                    impls.push(decl);
                    DeclKind::Impl
                })
            } else if self.at(&TokenKind::Effect) {
                // An `effect` block is a module item (spec 0037): it declares a
                // capitalized effect and desugars its operations into ordinary
                // functions/externs, which join the top-level collections. The
                // extern-canonicalization loop below stamps the externs with the
                // file's `module` (the canonical platform name stays
                // module-qualified, e.g. `io.write_stdout`).
                self.parse_effect_decl()
                    .map(|(decl, effect_fns, effect_externs)| {
                        effects.push(decl);
                        functions.extend(effect_fns);
                        externs.extend(effect_externs);
                        DeclKind::Effect
                    })
            } else {
                let is_public = self.eat(&TokenKind::Pub);
                if is_public && matches!(self.peek().kind, TokenKind::At(_)) {
                    Err(Error::diagnostic(
                        Diagnostic::new("Attribute after `pub`").label(
                            self.peek().span.clone(),
                            "attributes are placed before `pub` (spec 0039 R2)",
                        ),
                    ))
                } else if self.at(&TokenKind::Enum) {
                    self.parse_enum().map(|decl| {
                        enums.push(decl);
                        DeclKind::Enum
                    })
                } else if self.at(&TokenKind::Record) {
                    self.parse_record().map(|decl| {
                        records.push(decl);
                        DeclKind::Record
                    })
                } else if self.at(&TokenKind::Extern) {
                    self.parse_extern().map(|declaration| {
                        externs.push(declaration);
                        DeclKind::Extern
                    })
                } else {
                    self.parse_function(is_public).map(|function| {
                        functions.push(function);
                        DeclKind::Fn
                    })
                }
            };
            match result {
                Ok(kind) => apply_attributes(attrs, kind, &mut functions, &mut enums, errors),
                Err(error) => {
                    errors.push(error);
                    // The failed declaration may leave its type parameters
                    // installed; clear them so they don't leak into the next one.
                    self.type_params = Vec::new();
                    self.recover_to_top_level();
                }
            }
            self.skip_newlines();
        }
        // The declaring module qualifies each extern's canonical platform name,
        // and is the "owning module" for the orphan rule (spec 0020): a trait may
        // be implemented for a type only in the type's or the trait's module.
        // Functions record it as their extern-visibility domain (spec 0037): a
        // bare `extern`/`intrinsic` reference resolves only from the module that
        // declares it.
        for declaration in &mut externs {
            declaration.module = module.clone();
        }
        for function in &mut functions {
            function.declared_module = module.clone();
        }
        for decl in &mut enums {
            decl.module = module.clone();
        }
        for decl in &mut records {
            decl.module = module.clone();
        }
        for decl in &mut traits {
            decl.module = module.clone();
        }
        for decl in &mut impls {
            decl.module = module.clone();
            for method in &mut decl.methods {
                method.declared_module = module.clone();
            }
        }
        Program {
            module,
            imports,
            functions,
            externs,
            enums,
            records,
            traits,
            impls,
            effects,
        }
    }

    /// Skips past a failed top-level declaration so parsing resumes at the next
    /// one (spec 0033). Tokens are consumed until, at brace depth zero, a
    /// declaration-starting keyword appears at the start of a line; a stray `}`
    /// (the tail of the broken declaration's body) is consumed along the way.
    /// The first inspected token never stops the skip, so at least one token is
    /// always consumed and the parse loop makes progress.
    fn recover_to_top_level(&mut self) {
        // Resync to the next top-level declaration after a parse error (spec
        // 0033). A declaration keyword at brace depth 0 marks the boundary.
        // Newlines are deliberately not used as boundaries: an unterminated
        // `(`/`[` makes the lexer treat the newlines between declarations as
        // insignificant (spec 0034), so they may be absent from the stream. At
        // brace depth 0 a declaration keyword is never a lambda or a type, so
        // it is an unambiguous resync point.
        let mut depth = 0usize;
        while !self.at(&TokenKind::Eof) {
            match &self.peek().kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth = depth.saturating_sub(1),
                kind if depth == 0 && is_decl_start(kind) => return,
                _ => {}
            }
            self.bump();
        }
    }

    /// Parses a run of leading attributes (spec 0039): `@name` tokens, each on
    /// its own line (or inline) before a declaration. The reserved argument
    /// form `@name(...)` (R7) is reported and its group skipped, so parsing
    /// continues at the declaration.
    fn parse_attributes(&mut self, errors: &mut Vec<Error>) -> Vec<Attribute> {
        let mut attrs = Vec::new();
        while let TokenKind::At(name) = &self.peek().kind {
            let name = name.clone();
            let token = self.bump();
            let arg = if self.at(&TokenKind::LParen) {
                self.parse_attribute_arg(errors)
            } else {
                None
            };
            attrs.push(Attribute {
                name,
                arg,
                span: token.span,
            });
            self.skip_newlines();
        }
        attrs
    }

    /// Parses a parenthesized attribute argument (spec 0039 R7): a single string
    /// literal, as in `@lang("option")`. Called with the cursor on the opening
    /// `(`. Returns the string, or `None` (with an error pushed and the group
    /// skipped) when the group is not exactly one string literal.
    fn parse_attribute_arg(&mut self, errors: &mut Vec<Error>) -> Option<String> {
        let span = self.peek().span.clone();
        self.bump(); // `(`
        let value = if let TokenKind::String(text) = self.peek().kind.clone() {
            self.bump();
            Some(text)
        } else {
            None
        };
        if value.is_some() && self.at(&TokenKind::RParen) {
            self.bump(); // `)`
            return value;
        }
        errors.push(Error::diagnostic(
            Diagnostic::new("Invalid attribute argument").label(
                span,
                "an attribute argument must be a single string literal (spec 0039)",
            ),
        ));
        // Recover past the malformed group (the opening `(` is already consumed).
        let mut depth = 1usize;
        while depth > 0 && !self.at(&TokenKind::Eof) {
            match self.bump().kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth -= 1,
                _ => {}
            }
        }
        None
    }

    /// Rejects an attribute in a nested position (spec 0039 R2): attributes may
    /// only precede top-level declarations, never trait / impl methods or
    /// effect operations.
    fn reject_nested_attribute(&self) -> Result<()> {
        if matches!(self.peek().kind, TokenKind::At(_)) {
            return Err(Error::diagnostic(
                Diagnostic::new("Attribute in a nested position").label(
                    self.peek().span.clone(),
                    "attributes may only precede top-level declarations (spec 0039)",
                ),
            ));
        }
        Ok(())
    }

    fn parse_enum(&mut self) -> Result<EnumDecl> {
        self.expect(&TokenKind::Enum)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        // Type parameters (spec 0028); no bounds are allowed on a data type.
        let (type_params, bounds) = self.parse_type_params()?;
        if let Some(bound) = bounds.first() {
            return Err(Error::diagnostic(Diagnostic::new("Bound on a data type").label(
                bound.span.clone(),
                "enum type parameters cannot have trait bounds; the requirement belongs on the functions or impls that use the type",
            )));
        }
        // The type parameters are in scope while parsing the variant payloads, so
        // a payload type `T` resolves to `Type::Var("T")` rather than an enum name.
        self.type_params = type_params.clone();
        self.expect(&TokenKind::LBrace)?;
        let mut variants = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            let variant_span = self.peek().span.clone();
            let variant_name = self.expect_ident()?;
            let mut fields = Vec::new();
            if self.eat(&TokenKind::LParen) {
                if !self.at(&TokenKind::RParen) {
                    fields.push(self.parse_type()?);
                    // A trailing comma before the closer is allowed (spec 0034).
                    while self.eat(&TokenKind::Comma) {
                        if self.at(&TokenKind::RParen) {
                            break;
                        }
                        fields.push(self.parse_type()?);
                    }
                }
                self.expect(&TokenKind::RParen)?;
            }
            variants.push(EnumVariant {
                name: variant_name,
                name_span: variant_span,
                fields,
            });
            // Variants are separated by newlines and/or commas.
            self.eat(&TokenKind::Comma);
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        self.type_params = Vec::new();
        Ok(EnumDecl {
            name,
            name_span,
            type_params,
            // Stamped with the declaring module in `parse_program`.
            module: None,
            variants,
            // Set from `@lang("<role>")` by `apply_attributes` (spec 0039/0041).
            lang_item: None,
        })
    }

    /// Parses a `record` declaration (spec 0006): named, typed fields separated
    /// by newlines and/or commas. Records are non-generic in this first cut.
    fn parse_record(&mut self) -> Result<RecordDecl> {
        self.expect(&TokenKind::Record)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        let mut fields = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            let field_span = self.peek().span.clone();
            let field_name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type()?;
            fields.push(RecordFieldDef {
                name: field_name,
                name_span: field_span,
                ty,
            });
            self.eat(&TokenKind::Comma);
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(RecordDecl {
            name,
            name_span,
            // Stamped with the declaring module in `parse_program`.
            module: None,
            fields,
        })
    }

    /// Parses an optional `throws E` clause (spec 0008/0011). `throws Never` is
    /// non-throwing, equivalent to omitting the clause.
    fn parse_throws_clause(&mut self) -> Result<Option<Type>> {
        if self.eat(&TokenKind::Throws) {
            let ty = self.parse_type()?;
            if matches!(ty, Type::Never) {
                Ok(None)
            } else {
                Ok(Some(ty))
            }
        } else {
            Ok(None)
        }
    }

    fn parse_extern(&mut self) -> Result<Extern> {
        self.expect(&TokenKind::Extern)?;
        self.parse_extern_like(false)
    }

    /// Parses an `intrinsic fn` declaration (spec 0021): a pure, no-body
    /// primitive the backend inlines to a native instruction. Same shape as
    /// `extern fn` but tagged `is_intrinsic`.
    fn parse_intrinsic(&mut self) -> Result<Extern> {
        self.expect(&TokenKind::Intrinsic)?;
        self.parse_extern_like(true)
    }

    /// Parses the shared body of `extern fn` and `intrinsic fn` after the
    /// leading keyword: `fn name[<T, ...>](params) -> ret [throws] [uses]`. Type
    /// parameters are accepted only for a generic `intrinsic fn` (spec 0021);
    /// `parse_type_params` sets `self.type_params` so `T` parses as a
    /// `Type::Var` in the signature, mirroring `parse_function`.
    fn parse_extern_like(&mut self, is_intrinsic: bool) -> Result<Extern> {
        self.expect(&TokenKind::Fn)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        let (type_params, bounds) = self.parse_type_params()?;
        // Only a pure `intrinsic fn` may be generic (spec 0021). A platform
        // `extern fn` (spec 0013) is a concrete runtime call and is never
        // parameterized.
        if !is_intrinsic && !type_params.is_empty() {
            return Err(Error::diagnostic(
                Diagnostic::new("Generic extern function").label(
                    name_span.clone(),
                    "`extern fn` cannot be generic; only `intrinsic fn` may declare type parameters",
                ),
            ));
        }
        self.type_params = type_params.clone();
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let throws = self.parse_throws_clause()?;
        let effects = self.parse_effect_row()?;
        self.type_params = Vec::new();
        Ok(Extern {
            name,
            name_span,
            module: None,
            type_params,
            bounds,
            params,
            ret,
            throws,
            effects,
            is_intrinsic,
            effect_name: None,
        })
    }

    /// Parses an `effect` declaration (spec 0037): a capitalized, first-class
    /// effect that owns a set of operations, declared as a module item (like an
    /// enum). Each operation is desugared into an ordinary `fn`/`extern fn` that
    /// carries the effect implicitly in `uses { <Name> }`. Operations are
    /// stamped with `module_path = [Name]` and `effect_name = Some(Name)`, so
    /// `fn` operations are addressed as `Io.print(...)` (local or imported
    /// alike) while sibling operations of the same effect still reach each
    /// other by bare name. Returns the effect declaration and its operations.
    fn parse_effect_decl(&mut self) -> Result<(EffectDecl, Vec<Function>, Vec<Extern>)> {
        self.expect(&TokenKind::Effect)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        // An effect is a capitalized entity like a type (spec 0037), which is
        // also what keeps `Io.print(...)` distinguishable from a module-
        // qualified call at a glance.
        if !name.chars().next().is_some_and(char::is_uppercase) {
            return Err(Error::diagnostic(
                Diagnostic::new("Invalid effect name")
                    .label(
                        name_span.clone(),
                        "effect names start with an uppercase letter (spec 0037)",
                    )
                    .help(format!(
                        "Rename the effect, e.g. `effect {}`.",
                        capitalize(&name)
                    )),
            ));
        }
        // Every operation implicitly `uses { <Name> }` (spec 0037); authors must
        // not repeat it, and no operation may declare any other effect in v1.
        let effect_row = EffectRow::sorted(vec![name.clone()]);
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();
        let mut functions = Vec::new();
        let mut externs = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            self.reject_nested_attribute()?;
            if self.at(&TokenKind::Intrinsic) {
                // An `intrinsic fn` must be pure (spec 0021), which contradicts an
                // effect operation. Effects are backed by `extern fn` (spec 0013).
                return Err(Error::diagnostic(
                    Diagnostic::new("Intrinsic inside effect").label(
                        self.peek().span.clone(),
                        "an `effect` operation cannot be `intrinsic`; use `extern fn` or `fn`",
                    ),
                ));
            } else if self.at(&TokenKind::Extern) {
                let mut declaration = self.parse_extern()?;
                if !declaration.effects.effects.is_empty() {
                    return Err(explicit_uses_in_effect(&declaration.name_span, &name));
                }
                declaration.effects = effect_row.clone();
                declaration.effect_name = Some(name.clone());
                externs.push(declaration);
            } else {
                let is_public = self.eat(&TokenKind::Pub);
                let mut function = self.parse_function(is_public)?;
                // An effect operation may declare its own dependency effects
                // (spec 0049 PE2b: an inline default body can use other effects,
                // e.g. `fn info(msg) uses { Io } { ... }`) — this relaxes spec
                // 0037's original "implicit `uses { Name }`, no explicit uses"
                // and resolves 0036 Open Question 1. When it declares none it
                // implicitly depends only on its own effect (the old behaviour).
                // `function.effects` is the operation's *dependencies* (checked
                // against its body); *using* the operation contributes the
                // effect `Name` (the caller-facing signature, stamped in
                // `register_functions`).
                if function.effects.effects.is_empty() {
                    function.effects = effect_row.clone();
                }
                function.effect_name = Some(name.clone());
                function.module_path = vec![name.clone()];
                functions.push(function);
            }
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        Ok((EffectDecl { name, name_span }, functions, externs))
    }

    fn parse_import(&mut self) -> Result<Import> {
        let start = self.expect(&TokenKind::Import)?.span;
        // An import names a module (spec 0037): one segment for a sibling file
        // (`import geometry`), more for a package or nested path
        // (`import std.list`).
        let mut path = vec![self.expect_ident()?];
        while self.eat(&TokenKind::Dot) {
            path.push(self.expect_ident()?);
        }
        let end = self.previous_span();
        Ok(Import {
            path,
            span: start.merge(&end),
        })
    }

    fn parse_function(&mut self, is_public: bool) -> Result<Function> {
        self.expect(&TokenKind::Fn)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        // Type parameters (spec 0014) are in scope for the whole definition, so
        // `parse_type` resolves them to `Type::Var` throughout the signature and
        // body. Functions never nest, and a parse error aborts the whole parse,
        // so resetting on the success path is enough.
        let (type_params, bounds) = self.parse_type_params()?;
        self.type_params = type_params.clone();
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let throws = self.parse_throws_clause()?;
        let effects = self.parse_effect_row()?;
        let body = self.parse_block()?;
        self.type_params = Vec::new();
        Ok(Function {
            name,
            name_span,
            is_public,
            // The compilation root's own functions carry no qualifier;
            // `imports.rs` stamps `module_path` on functions pulled in by an
            // `import`, and `parse_effect_decl` stamps `[EffectName]` on effect
            // operations (spec 0037). `declared_module` is stamped by
            // `parse_program` once the file's `module` header is known.
            module_path: Vec::new(),
            declared_module: None,
            effect_name: None,
            type_params,
            bounds,
            params,
            ret,
            throws,
            effects,
            body,
            // Set by `apply_attributes` when the declaration carries `@test`.
            is_test: false,
        })
    }

    /// Parses an optional `<T, U: Bound + Bound2, ...>` type-parameter list
    /// (spec 0014; bounds are spec 0020). Returns the parameter names and their
    /// bounds. An empty vec when there is no list. An empty `<>` is rejected.
    /// The `+` between bounds is unambiguous with the arithmetic `+` because it
    /// only appears inside `< >`.
    fn parse_type_params(&mut self) -> Result<(Vec<String>, Vec<Bound>)> {
        if !self.eat(&TokenKind::Lt) {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut params = Vec::new();
        let mut bounds = Vec::new();
        let first_span = self.peek().span.clone();
        loop {
            let param_span = self.peek().span.clone();
            let name = self.expect_ident()?;
            if self.eat(&TokenKind::Colon) {
                let mut traits = vec![self.expect_ident()?];
                while self.eat(&TokenKind::Plus) {
                    traits.push(self.expect_ident()?);
                }
                bounds.push(Bound {
                    param: name.clone(),
                    traits,
                    span: param_span,
                });
            }
            params.push(name);
            // A trailing comma before the closer is allowed (spec 0034).
            if !self.eat(&TokenKind::Comma) || self.at(&TokenKind::Gt) {
                break;
            }
        }
        self.expect(&TokenKind::Gt)?;
        // Guard against duplicate names like `<T, T>`.
        let mut seen = std::collections::HashSet::new();
        for name in &params {
            if !seen.insert(name.clone()) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Duplicate type parameter").label(
                        first_span.clone(),
                        format!("type parameter `{name}` is declared more than once"),
                    ),
                ));
            }
        }
        Ok((params, bounds))
    }

    /// Parses a `trait Name { method signatures }` block (spec 0020). Each method
    /// is a signature with an optional default body. `Self` is in scope
    /// throughout, resolving to `Type::Var("Self")`.
    fn parse_trait(&mut self) -> Result<TraitDecl> {
        self.expect(&TokenKind::Trait)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LBrace)?;
        self.type_params = vec!["Self".to_string()];
        let mut methods = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            self.reject_nested_attribute()?;
            methods.push(self.parse_method_sig()?);
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        self.type_params = Vec::new();
        Ok(TraitDecl {
            name,
            name_span,
            // Stamped with the declaring module in `parse_program`.
            module: None,
            methods,
        })
    }

    /// Parses one trait method signature: `fn name(params) -> ret [throws] [uses]`
    /// followed by an optional `{ block }` default body.
    fn parse_method_sig(&mut self) -> Result<TraitMethodSig> {
        self.expect(&TokenKind::Fn)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let throws = self.parse_throws_clause()?;
        let effects = self.parse_effect_row()?;
        let default_body = if self.at(&TokenKind::LBrace) {
            Some(self.parse_block()?)
        } else {
            None
        };
        Ok(TraitMethodSig {
            name,
            name_span,
            params,
            ret,
            throws,
            effects,
            default_body,
        })
    }

    /// Parses an `impl [<params>] Trait for Type { methods }` block (spec 0020).
    /// The impl's own parameters and `Self` are in scope for the target type and
    /// every method.
    fn parse_impl(&mut self) -> Result<ImplDecl> {
        self.expect(&TokenKind::Impl)?;
        let (type_params, bounds) = self.parse_type_params()?;
        let trait_span = self.peek().span.clone();
        let trait_name = self.expect_ident()?;
        self.expect(&TokenKind::For)?;
        self.type_params = type_params.clone();
        self.type_params.push("Self".to_string());
        let target_start = self.peek().span.clone();
        let target = self.parse_type()?;
        let target_span = target_start.merge(&self.previous_span());
        self.expect(&TokenKind::LBrace)?;
        let mut methods = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            self.reject_nested_attribute()?;
            methods.push(self.parse_impl_method()?);
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        self.type_params = Vec::new();
        Ok(ImplDecl {
            trait_name,
            trait_span,
            target,
            target_span,
            type_params,
            bounds,
            // Stamped with the declaring module in `parse_program`.
            module: None,
            methods,
        })
    }

    /// Parses a method definition inside an `impl` block: like a function but
    /// with no type-parameter list of its own — it inherits `Self` and the
    /// impl's parameters, already installed in `self.type_params`.
    fn parse_impl_method(&mut self) -> Result<Function> {
        self.expect(&TokenKind::Fn)?;
        let name_span = self.peek().span.clone();
        let name = self.expect_ident()?;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let throws = self.parse_throws_clause()?;
        let effects = self.parse_effect_row()?;
        let body = self.parse_block()?;
        Ok(Function {
            name,
            name_span,
            is_public: false,
            module_path: Vec::new(),
            declared_module: None,
            effect_name: None,
            type_params: Vec::new(),
            bounds: Vec::new(),
            params,
            ret,
            throws,
            effects,
            body,
            is_test: false,
        })
    }

    fn parse_params(&mut self) -> Result<Vec<Param>> {
        let mut params = Vec::new();
        if self.at(&TokenKind::RParen) {
            return Ok(params);
        }
        loop {
            let name_span = self.peek().span.clone();
            let name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let ty = self.parse_type()?;
            params.push(Param {
                name,
                name_span,
                ty,
            });
            // A trailing comma before the closer is allowed (spec 0034).
            if !self.eat(&TokenKind::Comma) || self.at(&TokenKind::RParen) {
                break;
            }
        }
        Ok(params)
    }

    fn parse_type(&mut self) -> Result<Type> {
        let span = self.peek().span.clone();
        if self.eat(&TokenKind::LParen) {
            let mut params = Vec::new();
            if !self.at(&TokenKind::RParen) {
                params.push(self.parse_type()?);
                // A trailing comma before the closer is allowed (spec 0034).
                while self.eat(&TokenKind::Comma) {
                    if self.at(&TokenKind::RParen) {
                        break;
                    }
                    params.push(self.parse_type()?);
                }
            }
            self.expect(&TokenKind::RParen)?;
            if self.eat(&TokenKind::Arrow) {
                let ret = self.parse_type()?;
                let throws = self.parse_throws_clause()?;
                let effects = self.parse_effect_row()?;
                return Ok(Type::Function(FunctionType {
                    params,
                    ret: Box::new(ret),
                    throws: throws.map(Box::new),
                    effects,
                }));
            }
            return match params.len() {
                1 => Ok(params.remove(0)),
                _ => Err(Error::diagnostic(
                    Diagnostic::new("Expected function type")
                        .label(span, "parenthesized type lists need `-> ReturnType`"),
                )),
            };
        }
        let name = self.expect_ident()?;
        match name.as_str() {
            "Unit" => Ok(Type::Unit),
            "Bool" => Ok(Type::Bool),
            "Int" => Ok(Type::Int),
            "Float" => Ok(Type::Float),
            "String" => Ok(Type::String),
            "Char" => Ok(Type::Char),
            "Bytes" => Ok(Type::Bytes),
            "Array" => {
                self.expect(&TokenKind::Lt)?;
                let element = self.parse_type()?;
                self.expect(&TokenKind::Gt)?;
                Ok(Type::Array(Box::new(element)))
            }
            "Record" => Ok(Type::Record),
            "Never" => Ok(Type::Never),
            "Function" => Ok(Type::OpaqueFunction),
            // A name declared as a type parameter of the enclosing function
            // resolves to a type variable (spec 0014).
            _ if self.type_params.contains(&name) => Ok(Type::Var(name)),
            // Any other capitalized name refers to a declared enum type (spec
            // 0005), optionally with type arguments (spec 0028): `List<Int>`.
            // The name and arity are resolved and validated during type checking.
            _ => {
                let mut args = Vec::new();
                if self.eat(&TokenKind::Lt) {
                    args.push(self.parse_type()?);
                    // A trailing comma before the closer is allowed (spec 0034).
                    while self.eat(&TokenKind::Comma) {
                        if self.at(&TokenKind::Gt) {
                            break;
                        }
                        args.push(self.parse_type()?);
                    }
                    self.expect(&TokenKind::Gt)?;
                }
                Ok(Type::Enum(name, args))
            }
        }
    }

    fn parse_effect_row(&mut self) -> Result<EffectRow> {
        if !self.eat(&TokenKind::Uses) {
            return Ok(EffectRow::default());
        }
        self.expect(&TokenKind::LBrace)?;
        // The effect row's braces are list braces, not a block: newlines inside
        // are insignificant (spec 0034 G2). The lexer cannot tell them apart
        // from block braces, so the newlines are skipped here. Commas remain
        // the only separator.
        self.skip_newlines();
        let mut effects = Vec::new();
        if !self.at(&TokenKind::RBrace) {
            effects.push(self.parse_effect_name()?);
            self.skip_newlines();
            // A trailing comma before the closer is allowed (spec 0034).
            while self.eat(&TokenKind::Comma) {
                self.skip_newlines();
                if self.at(&TokenKind::RBrace) {
                    break;
                }
                effects.push(self.parse_effect_name()?);
                self.skip_newlines();
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(EffectRow::sorted(effects))
    }

    /// Parses an effect name in a `uses { ... }` row: a single identifier or
    /// a dotted `host.<name>` path (spec 0026).
    fn parse_effect_name(&mut self) -> Result<String> {
        let first = self.expect_ident()?;
        // Dotted paths are only valid for `host.<name>` capabilities.
        if first == "host" && self.at(&TokenKind::Dot) {
            self.eat(&TokenKind::Dot);
            let second = self.expect_ident()?;
            return Ok(format!("host.{second}"));
        }
        Ok(first)
    }

    fn parse_block(&mut self) -> Result<Block> {
        let start = self.expect(&TokenKind::LBrace)?.span;
        let mut items = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(Error::diagnostic(
                    Diagnostic::new("Unterminated block")
                        .label(self.peek().span.clone(), "block is missing a closing `}`"),
                ));
            }
            if self.eat(&TokenKind::Let) {
                let name_span = self.peek().span.clone();
                let name = self.expect_ident()?;
                let ty = if self.eat(&TokenKind::Colon) {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                self.expect(&TokenKind::Eq)?;
                let value = self.parse_expr()?;
                items.push(BlockItem::Let {
                    name,
                    name_span,
                    ty,
                    value,
                });
            } else {
                items.push(BlockItem::Expr(self.parse_expr()?));
            }
            self.skip_newlines();
        }
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok(Block {
            items,
            span: start.merge(&end),
        })
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_pipe()
    }

    /// `|>`, the pipeline operator (spec 0019): the weakest binary operator,
    /// left-associative. `lhs |> rhs` is a pure syntactic desugaring to a `Call`
    /// (see [`pipe_desugar`]), so no later stage sees a pipe node. Both sides are
    /// parsed at the next-higher precedence (`parse_or`), which makes every other
    /// operator bind tighter: `a + b |> f` is `f(a + b)`, and the left-fold of the
    /// `while` yields `a |> f |> g` == `g(f(a))`.
    fn parse_pipe(&mut self) -> Result<Expr> {
        let mut expr = self.parse_or()?;
        while self.eat_across_newlines(&TokenKind::PipeGt) {
            let right = self.parse_or()?;
            expr = pipe_desugar(expr, right);
        }
        Ok(expr)
    }

    /// `||`, the weakest binary operator (spec 0027). `a || b` desugars to
    /// `if a { true } else { b }`, which short-circuits `b`.
    fn parse_or(&mut self) -> Result<Expr> {
        let mut expr = self.parse_and()?;
        while self.eat_across_newlines(&TokenKind::PipePipe) {
            let right = self.parse_and()?;
            let span = expr.span().merge(&right.span());
            let then = Expr::Bool(true, span.clone());
            expr = if_desugar(expr, then, right, span);
        }
        Ok(expr)
    }

    /// `&&`, binding tighter than `||` (spec 0027). `a && b` desugars to
    /// `if a { b } else { false }`, which short-circuits `b`.
    fn parse_and(&mut self) -> Result<Expr> {
        let mut expr = self.parse_equality()?;
        while self.eat_across_newlines(&TokenKind::AmpAmp) {
            let right = self.parse_equality()?;
            let span = expr.span().merge(&right.span());
            let els = Expr::Bool(false, span.clone());
            expr = if_desugar(expr, right, els, span);
        }
        Ok(expr)
    }

    fn parse_equality(&mut self) -> Result<Expr> {
        let mut expr = self.parse_sum()?;
        loop {
            // Comparisons share one precedence level, left-associative (spec 0027).
            let op = if self.eat_across_newlines(&TokenKind::EqEq) {
                BinaryOp::Eq
            } else if self.eat_across_newlines(&TokenKind::Ne) {
                BinaryOp::Ne
            } else if self.eat_across_newlines(&TokenKind::Lt) {
                BinaryOp::Lt
            } else if self.eat_across_newlines(&TokenKind::Gt) {
                BinaryOp::Gt
            } else if self.eat_across_newlines(&TokenKind::Le) {
                BinaryOp::Le
            } else if self.eat_across_newlines(&TokenKind::Ge) {
                BinaryOp::Ge
            } else {
                break;
            };
            let right = self.parse_sum()?;
            let span = expr.span().merge(&right.span());
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_sum(&mut self) -> Result<Expr> {
        let mut expr = self.parse_product()?;
        loop {
            let op = if self.eat_across_newlines(&TokenKind::Plus) {
                BinaryOp::Add
            } else if self.eat_across_newlines(&TokenKind::PlusPlus) {
                BinaryOp::Concat
            } else if self.eat_across_newlines(&TokenKind::Minus) {
                BinaryOp::Sub
            } else {
                break;
            };
            let right = self.parse_product()?;
            let span = expr.span().merge(&right.span());
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_product(&mut self) -> Result<Expr> {
        let mut expr = self.parse_unary()?;
        loop {
            let op = if self.eat_across_newlines(&TokenKind::Star) {
                BinaryOp::Mul
            } else if self.eat_across_newlines(&TokenKind::Slash) {
                BinaryOp::Div
            } else if self.eat_across_newlines(&TokenKind::Percent) {
                BinaryOp::Rem
            } else {
                break;
            };
            let right = self.parse_unary()?;
            let span = expr.span().merge(&right.span());
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    /// Prefix `!` (spec 0027), the tightest-binding operator. `!e` desugars to
    /// `if e { false } else { true }`; there is no operator trait for it. Applies
    /// recursively so `!!e` parses.
    fn parse_unary(&mut self) -> Result<Expr> {
        if self.at(&TokenKind::Bang) {
            let bang = self.bump();
            let operand = self.parse_unary()?;
            let span = bang.span.merge(&operand.span());
            let then = Expr::Bool(false, span.clone());
            let els = Expr::Bool(true, span.clone());
            return Ok(if_desugar(operand, then, els, span));
        }
        self.parse_call()
    }

    fn parse_call(&mut self) -> Result<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.eat(&TokenKind::LParen) {
                let mut args = Vec::new();
                if !self.at(&TokenKind::RParen) {
                    args.push(self.parse_expr()?);
                    // A trailing comma before the closer is allowed (spec 0034).
                    while self.eat(&TokenKind::Comma) {
                        if self.at(&TokenKind::RParen) {
                            break;
                        }
                        args.push(self.parse_expr()?);
                    }
                }
                let end = self.expect(&TokenKind::RParen)?.span;
                let span = expr.span().merge(&end);
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args,
                    span,
                };
            } else if self.at(&TokenKind::Question) {
                // Postfix `?` propagates errors / `None` (spec 0011).
                let end = self.bump().span;
                let span = expr.span().merge(&end);
                expr = Expr::Question {
                    value: Box::new(expr),
                    span,
                };
            } else if self.at(&TokenKind::Dot) {
                // Postfix field access on a non-path expression (spec 0006),
                // e.g. `Http.get(url).body`. Identifier chains never reach
                // here: the primary consumed their dots into a `Path`.
                self.bump();
                let name_span = self.peek().span.clone();
                let name = self.expect_ident()?;
                let span = expr.span().merge(&name_span);
                expr = Expr::Field {
                    target: Box::new(expr),
                    name,
                    name_span,
                    span,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// Parses a record literal `Name { field: expr ... }` (spec 0006) after the
    /// leading capitalized name. Fields are separated by newlines and/or commas.
    fn parse_record_literal(&mut self, name: String, name_span: Span) -> Result<Expr> {
        self.expect(&TokenKind::LBrace)?;
        self.skip_newlines();
        let mut fields = Vec::new();
        while !self.at(&TokenKind::RBrace) {
            let field_span = self.peek().span.clone();
            let field_name = self.expect_ident()?;
            self.expect(&TokenKind::Colon)?;
            let value = self.parse_expr()?;
            fields.push((field_name, field_span, value));
            self.eat(&TokenKind::Comma);
            self.skip_newlines();
        }
        let end = self.expect(&TokenKind::RBrace)?.span;
        let span = name_span.merge(&end);
        Ok(Expr::RecordLiteral {
            name,
            name_span,
            fields,
            span,
        })
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.peek().kind.clone() {
            TokenKind::Int(value) => {
                let span = self.bump().span;
                Ok(Expr::Int(value, span))
            }
            TokenKind::Float(value) => {
                let span = self.bump().span;
                Ok(Expr::Float(value, span))
            }
            TokenKind::String(value) => {
                let span = self.bump().span;
                Ok(Expr::String(value, span))
            }
            TokenKind::Char(value) => {
                let span = self.bump().span;
                Ok(Expr::Char(value, span))
            }
            TokenKind::True => {
                let span = self.bump().span;
                Ok(Expr::Bool(true, span))
            }
            TokenKind::False => {
                let span = self.bump().span;
                Ok(Expr::Bool(false, span))
            }
            TokenKind::Ident(_) => {
                let span = self.peek().span.clone();
                let name = self.expect_ident()?;
                if name.chars().next().is_some_and(char::is_uppercase)
                    && self.at(&TokenKind::LBrace)
                {
                    // A capitalized name directly followed by `{` is a record
                    // literal (spec 0006): `User { id: 1 ... }`.
                    return self.parse_record_literal(name, span);
                }
                if self.at(&TokenKind::ColonColon) {
                    // A `::` type path: `Enum::Variant` (specs 0005/0018 R7).
                    // Resolved to an enum variant later; a trailing `(args)` is
                    // attached by `parse_call`. (The former `Char::from_code` /
                    // `String::from_char` conversions are now bare intrinsics,
                    // spec 0021.)
                    let mut segments = vec![name];
                    while self.eat(&TokenKind::ColonColon) {
                        segments.push(self.expect_ident()?);
                    }
                    let end = self.previous_span();
                    Ok(Expr::TypePath {
                        segments,
                        span: span.merge(&end),
                    })
                } else if self.at(&TokenKind::Dot) {
                    // A dotted path: `int.to_string`, `std.int.to_string`, a
                    // receiver call `recv.method`, or `Trait.method` (spec 0018).
                    // The path is parsed uniformly here; any trailing `(args)` is
                    // attached by `parse_call`, and the meaning is resolved later.
                    let mut segments = vec![name];
                    while self.eat(&TokenKind::Dot) {
                        segments.push(self.expect_ident()?);
                    }
                    let end = self.previous_span();
                    Ok(Expr::Path {
                        segments,
                        span: span.merge(&end),
                    })
                } else {
                    Ok(Expr::Var(name, span))
                }
            }
            TokenKind::Throw => {
                let start = self.bump().span;
                let value = self.parse_expr()?;
                let span = start.merge(&value.span());
                Ok(Expr::Throw {
                    value: Box::new(value),
                    span,
                })
            }
            TokenKind::Panic => {
                let start = self.bump().span;
                self.expect(&TokenKind::LParen)?;
                let message = self.parse_expr()?;
                let end = self.expect(&TokenKind::RParen)?.span;
                Ok(Expr::Panic {
                    message: Box::new(message),
                    span: start.merge(&end),
                })
            }
            TokenKind::Match => self.parse_match(),
            TokenKind::Try => self.parse_try(),
            TokenKind::If => self.parse_if(),
            TokenKind::Fn => self.parse_fn_expr(),
            TokenKind::LBracket => {
                let start = self.bump().span;
                let mut values = Vec::new();
                if !self.at(&TokenKind::RBracket) {
                    values.push(self.parse_expr()?);
                    // A trailing comma before the closer is allowed (spec 0034).
                    while self.eat(&TokenKind::Comma) {
                        if self.at(&TokenKind::RBracket) {
                            break;
                        }
                        values.push(self.parse_expr()?);
                    }
                }
                let end = self.expect(&TokenKind::RBracket)?.span;
                Ok(Expr::Array(values, start.merge(&end)))
            }
            TokenKind::LBrace => Ok(Expr::Block(self.parse_block()?)),
            TokenKind::LParen => {
                let start = self.bump().span;
                if self.eat(&TokenKind::RParen) {
                    Ok(Expr::Unit(start))
                } else {
                    let expr = self.parse_expr()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(expr)
                }
            }
            _ => Err(Error::diagnostic(
                Diagnostic::new("Expected an expression")
                    .label(self.peek().span.clone(), "expected an expression here"),
            )),
        }
    }

    fn parse_fn_expr(&mut self) -> Result<Expr> {
        let start = self.expect(&TokenKind::Fn)?.span;
        self.expect(&TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen)?;
        self.expect(&TokenKind::Arrow)?;
        let ret = self.parse_type()?;
        let throws = self.parse_throws_clause()?;
        let effects = self.parse_effect_row()?;
        let body = self.parse_block()?;
        let span = start.merge(&body.span);
        Ok(Expr::Fn {
            params,
            ret,
            throws,
            effects,
            body,
            span,
        })
    }

    fn parse_if(&mut self) -> Result<Expr> {
        let start = self.expect(&TokenKind::If)?.span;
        let cond = self.parse_expr()?;
        let then = self.parse_block()?;
        self.expect(&TokenKind::Else)?;
        // `else if …` (spec 0015): parse the trailing `if` as a nested `if`
        // expression wrapped in a single-item block, so a chain of `else if`
        // desugars to right-nested `if`s. `else { … }` stays a plain block.
        let els = if self.at(&TokenKind::If) {
            block_of(self.parse_if()?)
        } else {
            self.parse_block()?
        };
        let span = start.merge(&els.span);
        Ok(Expr::If {
            cond: Box::new(cond),
            then,
            els,
            span,
        })
    }

    fn parse_match(&mut self) -> Result<Expr> {
        let start = self.expect(&TokenKind::Match)?.span;
        let scrutinee = self.parse_expr()?;
        self.expect(&TokenKind::LBrace)?;
        let arms = self.parse_match_arms()?;
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span: start.merge(&end),
        })
    }

    fn parse_try(&mut self) -> Result<Expr> {
        let start = self.expect(&TokenKind::Try)?.span;
        let body = self.parse_block()?;
        self.expect(&TokenKind::Catch)?;
        self.expect(&TokenKind::LBrace)?;
        let arms = self.parse_match_arms()?;
        let end = self.expect(&TokenKind::RBrace)?.span;
        Ok(Expr::Try {
            body,
            arms,
            span: start.merge(&end),
        })
    }

    fn parse_match_arms(&mut self) -> Result<Vec<MatchArm>> {
        let mut arms = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::RBrace) {
            let pattern = self.parse_pattern()?;
            let guard = if self.eat(&TokenKind::If) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(&TokenKind::Arrow)?;
            let body = self.parse_expr()?;
            let span = pattern_span(&pattern).merge(&body.span());
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                span,
            });
            self.eat(&TokenKind::Comma);
            self.skip_newlines();
        }
        Ok(arms)
    }

    fn parse_pattern(&mut self) -> Result<Pattern> {
        let span = self.peek().span.clone();
        let name = self.expect_ident()?;
        if name == "_" {
            return Ok(Pattern::Wildcard(span));
        }
        // A lowercase-leading name binds the whole scrutinee (catch-all).
        if name.chars().next().is_some_and(char::is_lowercase) {
            return Ok(Pattern::Binding { name, span });
        }
        // An uppercase-leading name is a variant, optionally `Enum::Variant`.
        let (enum_name, variant) = if self.eat(&TokenKind::ColonColon) {
            (Some(name), self.expect_ident()?)
        } else {
            (None, name)
        };
        let mut fields = Vec::new();
        if self.eat(&TokenKind::LParen) {
            if !self.at(&TokenKind::RParen) {
                fields.push(self.parse_field_binding()?);
                // A trailing comma before the closer is allowed (spec 0034).
                while self.eat(&TokenKind::Comma) {
                    if self.at(&TokenKind::RParen) {
                        break;
                    }
                    fields.push(self.parse_field_binding()?);
                }
            }
            self.expect(&TokenKind::RParen)?;
        }
        let end = self.previous_span();
        Ok(Pattern::Variant {
            enum_name,
            variant,
            fields,
            span: span.merge(&end),
        })
    }

    fn parse_field_binding(&mut self) -> Result<FieldBinding> {
        let name = self.expect_ident()?;
        if name == "_" {
            Ok(FieldBinding::Ignore)
        } else {
            Ok(FieldBinding::Name(name))
        }
    }

    fn skip_newlines(&mut self) {
        while self.at(&TokenKind::Newline) {
            self.bump();
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
                    .label(self.peek().span.clone(), "expected a name here"),
            )),
        }
    }

    fn parse_path_name(&mut self) -> Result<String> {
        let mut parts = vec![self.expect_ident()?];
        while self.eat(&TokenKind::Dot) {
            parts.push(self.expect_ident()?);
        }
        Ok(parts.join("."))
    }

    fn expect(&mut self, expected: &TokenKind) -> Result<Token> {
        if self.at(expected) {
            Ok(self.bump())
        } else {
            Err(Error::diagnostic(
                Diagnostic::new("Unexpected token")
                    .label(self.peek().span.clone(), format!("expected `{expected:?}`")),
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

    /// Like [`eat`], but tolerates newlines before the operator: a binary
    /// operator written at the start of the next line continues the current
    /// expression (spec 0034 G7, issue #62), so a pipeline `xs\n  |> f\n  |> g`
    /// parses as one expression. No binary operator can begin a statement — the
    /// language has no prefix `+`/`-`/`|>` (only `!`) — so consuming across a
    /// newline never merges two independent statements. On no match the position
    /// (including any skipped newlines) is fully restored, leaving the newline
    /// available as a statement separator.
    fn eat_across_newlines(&mut self, kind: &TokenKind) -> bool {
        let saved = self.current;
        self.skip_newlines();
        if self.eat(kind) {
            true
        } else {
            self.current = saved;
            false
        }
    }

    fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(&self.peek().kind) == std::mem::discriminant(kind)
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.current].clone();
        self.current += 1;
        token
    }

    fn previous_span(&self) -> Span {
        self.tokens[self.current.saturating_sub(1)].span.clone()
    }
}

/// Whether `kind` can start a top-level declaration — the synchronization
/// points for parser recovery (spec 0033).
fn is_decl_start(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Fn
            | TokenKind::Pub
            | TokenKind::Enum
            | TokenKind::Trait
            | TokenKind::Impl
            | TokenKind::Effect
            | TokenKind::Extern
            | TokenKind::Intrinsic
            | TokenKind::Import
            | TokenKind::At(_)
    )
}

/// A parsed `@name` or `@name("arg")` attribute (spec 0039), validated against
/// the declaration it precedes by `apply_attributes`.
struct Attribute {
    name: String,
    /// The string-literal argument of `@name("arg")` (spec 0039 R7), or `None`
    /// for the bare `@name` form. Which attributes may/must take an argument is
    /// enforced in `apply_attributes` (only `@lang` does, spec 0041).
    arg: Option<String>,
    span: Span,
}

/// The kind of top-level declaration just parsed, for attribute applicability
/// checks (spec 0039 R4–R6).
#[derive(Clone, Copy)]
enum DeclKind {
    Import,
    Extern,
    Enum,
    Record,
    Trait,
    Impl,
    Effect,
    Fn,
}

impl DeclKind {
    fn describe(self) -> &'static str {
        match self {
            DeclKind::Import => "an `import`",
            DeclKind::Extern => "an `extern` declaration",
            DeclKind::Enum => "an `enum`",
            DeclKind::Record => "a `record`",
            DeclKind::Trait => "a `trait`",
            DeclKind::Impl => "an `impl` block",
            DeclKind::Effect => "an `effect` declaration",
            DeclKind::Fn => "a function",
        }
    }
}

/// Validates the attributes collected before a declaration (spec 0039): the
/// recognized set is closed (R4), an unknown name is an error (R5), a duplicate
/// is an error (R3), each recognized attribute checks the declaration kind it
/// applies to (R6), and only argument-taking attributes may carry an argument
/// (R7). `@test` (spec 0040 T1) marks a top-level `fn` as a test; `@lang`
/// (spec 0041) binds a top-level `enum` to a lang-item role.
fn apply_attributes(
    attrs: Vec<Attribute>,
    kind: DeclKind,
    functions: &mut [Function],
    enums: &mut [EnumDecl],
    errors: &mut Vec<Error>,
) {
    let mut seen = std::collections::HashSet::new();
    for attr in attrs {
        if !seen.insert(attr.name.clone()) {
            errors.push(Error::diagnostic(
                Diagnostic::new("Duplicate attribute").label(
                    attr.span.clone(),
                    format!(
                        "attribute `@{}` is applied more than once (spec 0039)",
                        attr.name
                    ),
                ),
            ));
            continue;
        }
        if matches!(kind, DeclKind::Import) {
            errors.push(Error::diagnostic(
                Diagnostic::new("Attribute on an import").label(
                    attr.span.clone(),
                    "attributes cannot be applied to an `import` (spec 0039)",
                ),
            ));
            continue;
        }
        match attr.name.as_str() {
            "test" => {
                // `@test` takes no arguments (spec 0039 R7 / 0040).
                if attr.arg.is_some() {
                    errors.push(Error::diagnostic(
                        Diagnostic::new("Attribute takes no arguments").label(
                            attr.span.clone(),
                            "`@test` takes no arguments (spec 0040)",
                        ),
                    ));
                    continue;
                }
                match kind {
                    DeclKind::Fn => {
                        if let Some(function) = functions.last_mut() {
                            function.is_test = true;
                        }
                    }
                    _ => errors.push(Error::diagnostic(
                        Diagnostic::new("Attribute does not apply here").label(
                            attr.span.clone(),
                            format!(
                                "`@test` may only be applied to a top-level `fn`, not {} (spec 0040)",
                                kind.describe()
                            ),
                        ),
                    )),
                }
            }
            "lang" => match kind {
                // `@lang("<role>")` binds this enum to a lang-item role. The role
                // set is closed (spec 0041 L1); only `option` is recognized so
                // far (spec 0042). The role is stored on the enum and consumed
                // where the compiler builds the lang-item table.
                DeclKind::Enum => match attr.arg.as_deref() {
                    Some("option") => {
                        if let Some(decl) = enums.last_mut() {
                            decl.lang_item = Some("option".to_string());
                        }
                    }
                    Some(role) => errors.push(Error::diagnostic(
                        Diagnostic::new("Unknown lang-item role").label(
                            attr.span.clone(),
                            format!(
                                "unknown lang-item role `\"{role}\"`; recognized roles: `\"option\"` (spec 0042)"
                            ),
                        ),
                    )),
                    None => errors.push(Error::diagnostic(
                        Diagnostic::new("Missing lang-item role").label(
                            attr.span.clone(),
                            "`@lang` requires a role name, e.g. `@lang(\"option\")` (spec 0041)",
                        ),
                    )),
                },
                _ => errors.push(Error::diagnostic(
                    Diagnostic::new("Attribute does not apply here").label(
                        attr.span.clone(),
                        format!(
                            "`@lang` may only be applied to a top-level `enum`, not {} (spec 0041)",
                            kind.describe()
                        ),
                    ),
                )),
            },
            other => errors.push(Error::diagnostic(
                Diagnostic::new("Unknown attribute").label(
                    attr.span.clone(),
                    format!(
                        "unknown attribute `@{other}`; recognized attributes: `@test` (spec 0040), `@lang` (spec 0041)"
                    ),
                ),
            )),
        }
    }
}

/// Uppercases the first letter, for the effect-name rename suggestion.
fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The error for an explicit `uses { ... }` on an operation inside an `effect`
/// block (spec 0036): the effect is implicit, so writing it is redundant.
fn explicit_uses_in_effect(name_span: &Span, effect: &str) -> Error {
    Error::diagnostic(Diagnostic::new("Redundant effect on operation").label(
        name_span.clone(),
        format!("an operation of `effect {effect}` already uses `{{ {effect} }}`; remove the `uses` clause"),
    ))
}

fn pattern_span(pattern: &Pattern) -> Span {
    match pattern {
        Pattern::Variant { span, .. } | Pattern::Binding { span, .. } => span.clone(),
        Pattern::Wildcard(span) => span.clone(),
    }
}

/// Wraps a bare expression as a single-item block, for a desugared `if` branch.
fn block_of(expr: Expr) -> Block {
    let span = expr.span();
    Block {
        items: vec![BlockItem::Expr(expr)],
        span,
    }
}

/// Builds `if cond { then } else { els }` from bare branch expressions. The
/// logical operators `&& || !` (spec 0027) are Bool-only control constructs, so
/// they desugar to `if` rather than to a trait method.
fn if_desugar(cond: Expr, then: Expr, els: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(cond),
        then: block_of(then),
        els: block_of(els),
        span,
    }
}

/// Desugars a single `lhs |> rhs` pipe (spec 0019) into an ordinary `Call`.
/// The shape of `rhs` decides the insertion:
///
/// - `Question { value }` (a trailing `?`, spec 0011): the `?` applies *after*
///   insertion (P4), so recurse into `value` and re-wrap — `lhs |> g?` becomes
///   `(lhs |> g)?`.
/// - `Call { callee, args }`: first-argument insertion (P2) — `lhs |> f(a, b)`
///   becomes `f(lhs, a, b)`.
/// - anything else `e`: a bare function value (P3) — `lhs |> e` becomes `e(lhs)`.
///
/// `lhs` is only moved into argument position, never cloned, so it is evaluated
/// exactly once (P5).
fn pipe_desugar(lhs: Expr, rhs: Expr) -> Expr {
    match rhs {
        Expr::Question { value, span } => {
            let span = lhs.span().merge(&span);
            let inner = pipe_desugar(lhs, *value);
            Expr::Question {
                value: Box::new(inner),
                span,
            }
        }
        Expr::Call { callee, args, span } => {
            let span = lhs.span().merge(&span);
            let mut new_args = Vec::with_capacity(args.len() + 1);
            new_args.push(lhs);
            new_args.extend(args);
            Expr::Call {
                callee,
                args: new_args,
                span,
            }
        }
        callee => {
            let span = lhs.span().merge(&callee.span());
            Expr::Call {
                callee: Box::new(callee),
                args: vec![lhs],
                span,
            }
        }
    }
}

#[allow(dead_code)]
fn _span(_: &Span) {}
