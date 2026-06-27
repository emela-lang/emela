use crate::ast::{
    BinaryOp, Block, BlockItem, Expr, Function, MatchArm, Pattern, PrimType, Program,
};
use crate::error::{Error, Result};
use crate::lexer::{Token, TokenKind};

pub(crate) struct Parser {
    tokens: Vec<Token>,
    current: usize,
}

impl Parser {
    pub(crate) fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, current: 0 }
    }

    pub(crate) fn parse_program(&mut self) -> Result<Program> {
        let mut functions = Vec::new();
        self.skip_newlines();
        while !self.at(&TokenKind::Eof) {
            self.parse_attributes()?;
            functions.push(self.parse_function()?);
            self.skip_newlines();
        }
        Ok(Program { functions })
    }

    fn parse_attributes(&mut self) -> Result<()> {
        loop {
            self.skip_newlines();
            if !self.at(&TokenKind::Hash) {
                return Ok(());
            }
            self.bump();
            self.expect(&TokenKind::LBracket)?;
            self.expect_ident()?;
            if self.eat(&TokenKind::LParen) {
                if !self.at(&TokenKind::RParen) {
                    self.expect_ident()?;
                    while self.eat(&TokenKind::Comma) {
                        self.expect_ident()?;
                    }
                }
                self.expect(&TokenKind::RParen)?;
            }
            self.expect(&TokenKind::RBracket)?;
        }
    }

    fn parse_function(&mut self) -> Result<Function> {
        self.expect(&TokenKind::Fn)?;
        let name = self.parse_function_name()?;
        self.expect(&TokenKind::LParen)?;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            params.push(self.expect_ident()?);
            while self.eat(&TokenKind::Comma) {
                params.push(self.expect_ident()?);
            }
        }
        self.expect(&TokenKind::RParen)?;
        let return_annotation = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Ok(Function {
            name,
            params,
            return_annotation,
            body,
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
                return Err(Error::new("unterminated block"));
            }

            if let TokenKind::Ident(name) = self.peek().kind.clone() {
                if self
                    .peek_n(1)
                    .is_some_and(|token| token.kind == TokenKind::Eq)
                {
                    self.bump();
                    self.bump();
                    let expr = self.parse_expr()?;
                    items.push(BlockItem::Binding { name, expr });
                } else {
                    items.push(BlockItem::Expr(self.parse_expr()?));
                }
            } else {
                items.push(BlockItem::Expr(self.parse_expr()?));
            }
            self.skip_newlines();
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(Block { items })
    }

    fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_equality()
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
                self.expect(&TokenKind::LParen)?;
                let args = self.parse_argument_list()?;
                self.expect(&TokenKind::RParen)?;
                expr = Expr::MethodCall {
                    receiver: Box::new(expr),
                    name,
                    args,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.peek().kind.clone() {
            TokenKind::Int(value) => {
                self.bump();
                Ok(Expr::Int(value))
            }
            TokenKind::True => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            TokenKind::False => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            TokenKind::Ident(_) => {
                let name = self.parse_function_name()?;
                if self.eat(&TokenKind::LParen) {
                    let args = self.parse_argument_list()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr::Call { name, args })
                } else {
                    if name.ends_with('!') {
                        return Err(Error::new("local variable names cannot end with !"));
                    }
                    Ok(Expr::Var(name))
                }
            }
            TokenKind::Match => {
                self.bump();
                let scrutinee = self.parse_expr()?;
                self.expect(&TokenKind::LBrace)?;
                let mut arms = Vec::new();
                self.skip_newlines();
                while !self.at(&TokenKind::RBrace) {
                    if self.at(&TokenKind::Eof) {
                        return Err(Error::new("unterminated match expression"));
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
                })
            }
            TokenKind::LBrace => Ok(Expr::Block(self.parse_block()?)),
            TokenKind::LParen => {
                self.bump();
                if self.eat(&TokenKind::RParen) {
                    Ok(Expr::Unit)
                } else {
                    let expr = self.parse_expr()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(expr)
                }
            }
            _ => Err(Error::new(format!(
                "expected expression at byte {}",
                self.peek().pos
            ))),
        }
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

    fn parse_type(&mut self) -> Result<PrimType> {
        let name = self.expect_ident()?;
        match name.as_str() {
            "I32" | "i32" => Ok(PrimType::I32),
            "Bool" | "bool" => Ok(PrimType::Bool),
            "Unit" | "unit" => Ok(PrimType::Unit),
            _ => Err(Error::new(format!("unknown type `{name}`"))),
        }
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
            _ => Err(Error::new(format!(
                "expected pattern at byte {}",
                self.peek().pos
            ))),
        }
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.peek().kind.clone() {
            TokenKind::Ident(name) => {
                self.bump();
                Ok(name)
            }
            _ => Err(Error::new(format!(
                "expected identifier at byte {}",
                self.peek().pos
            ))),
        }
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
            Err(Error::new(format!(
                "expected {:?} at byte {}",
                kind,
                self.peek().pos
            )))
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
}
