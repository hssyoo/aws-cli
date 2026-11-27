//! `jmespath/parser.py`: Pratt's top-down operator precedence parser,
//! token for token, to the AST of `jmespath/ast.py`, with its
//! `ParseError`s.
use crate::error::Error;
use crate::lexer::{Kind, Token, Value};
use crate::runtime::Runtime;

/// `Parser._PROJECTION_STOP`.
const PROJECTION_STOP: u8 = 10;

/// A comparator's operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparator {
    Eq,
    Ne,
    Lt,
    Gt,
    Lte,
    Gte,
}

impl Comparator {
    /// Python's spelling of the operator.
    pub(crate) const fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Gt => ">",
            Self::Lte => "<=",
            Self::Gte => ">=",
        }
    }
}

/// An AST node, as `jmespath/ast.py` builds them.
#[derive(Debug)]
pub enum Node<V> {
    /// A dict's member, by name in `View::String`'s encoding.
    Field(Box<[u8]>),
    Subexpression(Vec<Node<V>>),
    IndexExpression(Vec<Node<V>>),
    Index(i64),
    Slice([Option<i64>; 3]),
    /// A list projection: the left side's elements each through the right.
    Projection(Box<Node<V>>, Box<Node<V>>),
    /// `*`: a dict's values each through the right side.
    ValueProjection(Box<Node<V>>, Box<Node<V>>),
    /// `[?condition]`: left, right, condition.
    FilterProjection(Box<[Node<V>; 3]>),
    Flatten(Box<Node<V>>),
    Identity,
    Current,
    Literal(V),
    MultiSelectList(Vec<Node<V>>),
    MultiSelectDict(Vec<(Box<[u8]>, Node<V>)>),
    Or(Box<Node<V>>, Box<Node<V>>),
    And(Box<Node<V>>, Box<Node<V>>),
    Not(Box<Node<V>>),
    Pipe(Box<Node<V>>, Box<Node<V>>),
    Comparator(Comparator, Box<Node<V>>, Box<Node<V>>),
    Function(Box<str>, Vec<Node<V>>),
    Expref(Box<Node<V>>),
}

struct Parser<'r, R: Runtime> {
    runtime: &'r R,
    expression: &'r str,
    tokens: Vec<Token<R::Value>>,
    index: usize,
}

type Parsed<V> = Result<Node<V>, Error>;

/// `Parser().parse(expression)`.
pub(crate) fn parse<R: Runtime>(runtime: &R, expression: &str) -> Parsed<R::Value> {
    let tokens = crate::lexer::tokenize(runtime, expression)?;
    let mut parser = Parser {
        runtime,
        expression,
        tokens,
        index: 0,
    };
    let node = parser.expression(0)?;
    if parser.current() != Kind::Eof {
        let token = parser.token(0);
        let value = parser.display(token);
        return Err(parser.error_at(token, &format!("Unexpected token: {value}")));
    }
    Ok(node)
}

impl<R: Runtime> Parser<'_, R> {
    /// `_lookahead_token(offset)`; past either end, the `eof` token.
    fn token(&self, offset: isize) -> &Token<R::Value> {
        let last = self.tokens.len() - 1;
        let at = self
            .index
            .checked_add_signed(offset)
            .map_or(last, |at| at.min(last));
        &self.tokens[at]
    }

    fn current(&self) -> Kind {
        self.token(0).kind
    }

    fn advance(&mut self) {
        self.index += 1;
    }

    /// `str(token['value'])`.
    fn display(&self, token: &Token<R::Value>) -> String {
        match &token.value {
            Value::Text(text) => String::from_utf8_lossy(text).into_owned(),
            Value::Number(_, exact) => exact.clone(),
            Value::Json(value) => self.runtime.str(value),
        }
    }

    fn error_at(&self, token: &Token<R::Value>, reason: &str) -> Error {
        self.error(token.start, token, reason)
    }

    fn error(&self, position: usize, token: &Token<R::Value>, reason: &str) -> Error {
        Error::parse(
            self.expression,
            position,
            &self.display(token),
            token.kind.name(),
            reason,
        )
    }

    /// `_raise_parse_error_maybe_eof`.
    fn expected(&self, expected: &str) -> Error {
        let token = self.token(0);
        if token.kind == Kind::Eof {
            return Error::incomplete(self.expression);
        }
        self.error_at(
            token,
            &format!("Expecting: {expected}, got: {}", token.kind.name()),
        )
    }

    fn matches(&mut self, kind: Kind) -> Result<(), Error> {
        if self.current() != kind {
            return Err(self.expected(kind.name()));
        }
        self.advance();
        Ok(())
    }

    fn expression(&mut self, binding_power: u8) -> Parsed<R::Value> {
        let index = self.index;
        self.advance();
        let mut left = self.nud(index)?;
        while binding_power < self.current().binding_power() {
            let kind = self.current();
            self.advance();
            left = self.led(kind, left)?;
        }
        Ok(left)
    }

    fn nud(&mut self, index: usize) -> Parsed<R::Value> {
        let token = &self.tokens[index.min(self.tokens.len() - 1)];
        Ok(match token.kind {
            Kind::Literal => match &token.value {
                Value::Json(value) => Node::Literal(value.clone()),
                Value::Text(text) => Node::Literal(self.runtime.string(text.clone())),
                Value::Number(..) => Node::Identity,
            },
            Kind::UnquotedIdentifier => Node::Field(name(&token.value)),
            Kind::QuotedIdentifier => {
                let field = Node::Field(name(&token.value));
                if self.current() == Kind::Lparen {
                    let reason = "Quoted identifier not allowed for function names.";
                    return Err(self.error(0, self.token(0), reason));
                }
                field
            }
            Kind::Star => {
                let right = if self.current() == Kind::Rbracket {
                    Node::Identity
                } else {
                    self.projection_rhs(Kind::Star.binding_power())?
                };
                Node::ValueProjection(Box::new(Node::Identity), Box::new(right))
            }
            Kind::Filter => self.filter(Node::Identity)?,
            Kind::Lbrace => self.multi_select_dict()?,
            Kind::Lparen => {
                let expression = self.expression(0)?;
                self.matches(Kind::Rparen)?;
                expression
            }
            Kind::Flatten => {
                let right = self.projection_rhs(Kind::Flatten.binding_power())?;
                Node::Projection(
                    Box::new(Node::Flatten(Box::new(Node::Identity))),
                    Box::new(right),
                )
            }
            Kind::Not => Node::Not(Box::new(self.expression(Kind::Not.binding_power())?)),
            Kind::Lbracket => match self.current() {
                Kind::Number | Kind::Colon => {
                    let right = self.index_expression()?;
                    self.project_if_slice(Node::Identity, right)?
                }
                Kind::Star if self.token(1).kind == Kind::Rbracket => {
                    self.advance();
                    self.advance();
                    let right = self.projection_rhs(Kind::Star.binding_power())?;
                    Node::Projection(Box::new(Node::Identity), Box::new(right))
                }
                _ => self.multi_select_list()?,
            },
            Kind::Current => Node::Current,
            Kind::Expref => Node::Expref(Box::new(self.expression(Kind::Expref.binding_power())?)),
            Kind::Eof => return Err(Error::incomplete(self.expression)),
            _ => return Err(self.error_at(token, "invalid token")),
        })
    }

    fn led(&mut self, kind: Kind, left: Node<R::Value>) -> Parsed<R::Value> {
        Ok(match kind {
            Kind::Dot => {
                if self.current() == Kind::Star {
                    self.advance();
                    let right = self.projection_rhs(Kind::Dot.binding_power())?;
                    return Ok(Node::ValueProjection(Box::new(left), Box::new(right)));
                }
                let right = self.dot_rhs(Kind::Dot.binding_power())?;
                match left {
                    Node::Subexpression(mut children) => {
                        children.push(right);
                        Node::Subexpression(children)
                    }
                    left => Node::Subexpression(vec![left, right]),
                }
            }
            Kind::Pipe => Node::Pipe(Box::new(left), Box::new(self.expression(1)?)),
            Kind::Or => Node::Or(Box::new(left), Box::new(self.expression(2)?)),
            Kind::And => Node::And(Box::new(left), Box::new(self.expression(3)?)),
            Kind::Lparen => self.function(left)?,
            Kind::Filter => self.filter(left)?,
            Kind::Eq | Kind::Ne | Kind::Gt | Kind::Gte | Kind::Lt | Kind::Lte => {
                self.comparator(kind, left)?
            }
            Kind::Flatten => {
                let right = self.projection_rhs(Kind::Flatten.binding_power())?;
                Node::Projection(Box::new(Node::Flatten(Box::new(left))), Box::new(right))
            }
            Kind::Lbracket => match self.current() {
                Kind::Number | Kind::Colon => {
                    let right = self.index_expression()?;
                    match left {
                        Node::IndexExpression(mut children) => {
                            children.push(right);
                            Node::IndexExpression(children)
                        }
                        left => self.project_if_slice(left, right)?,
                    }
                }
                _ => {
                    self.matches(Kind::Star)?;
                    self.matches(Kind::Rbracket)?;
                    let right = self.projection_rhs(Kind::Star.binding_power())?;
                    Node::Projection(Box::new(left), Box::new(right))
                }
            },
            _ => {
                // A token with a binding power but no `led`: the one before
                // the current is the one that failed.
                self.index -= 1;
                return Err(self.error_at(self.token(0), "invalid token"));
            }
        })
    }

    /// `_token_led_lparen`: a call of the function `left` names.
    fn function(&mut self, left: Node<R::Value>) -> Parsed<R::Value> {
        let Node::Field(name) = left else {
            let token = self.token(-2);
            let reason = format!("Invalid function name '{}'", self.display(token));
            return Err(self.error_at(token, &reason));
        };
        let mut args = Vec::new();
        while self.current() != Kind::Rparen {
            let expression = self.expression(0)?;
            if self.current() == Kind::Comma {
                self.matches(Kind::Comma)?;
            }
            args.push(expression);
        }
        self.matches(Kind::Rparen)?;
        Ok(Node::Function(String::from_utf8_lossy(&name).into(), args))
    }

    fn index_expression(&mut self) -> Parsed<R::Value> {
        if self.current() == Kind::Colon || self.token(1).kind == Kind::Colon {
            return self.slice();
        }
        let index = match self.token(0).value {
            Value::Number(index, _) => index,
            _ => 0,
        };
        self.advance();
        self.matches(Kind::Rbracket)?;
        Ok(Node::Index(index))
    }

    fn slice(&mut self) -> Parsed<R::Value> {
        let mut parts = [None; 3];
        let mut index = 0;
        while self.current() != Kind::Rbracket && index < 3 {
            match (self.current(), &self.token(0).value) {
                (Kind::Colon, _) => {
                    index += 1;
                    if index == 3 {
                        return Err(self.error_at(self.token(0), "syntax error"));
                    }
                    self.advance();
                }
                (Kind::Number, Value::Number(value, _)) => {
                    parts[index] = Some(*value);
                    self.advance();
                }
                _ => return Err(self.error_at(self.token(0), "syntax error")),
            }
        }
        self.matches(Kind::Rbracket)?;
        Ok(Node::Slice(parts))
    }

    fn project_if_slice(
        &mut self,
        left: Node<R::Value>,
        right: Node<R::Value>,
    ) -> Parsed<R::Value> {
        let slice = matches!(right, Node::Slice(_));
        let index = Node::IndexExpression(vec![left, right]);
        if !slice {
            return Ok(index);
        }
        let right = self.projection_rhs(Kind::Star.binding_power())?;
        Ok(Node::Projection(Box::new(index), Box::new(right)))
    }

    fn filter(&mut self, left: Node<R::Value>) -> Parsed<R::Value> {
        let condition = self.expression(0)?;
        self.matches(Kind::Rbracket)?;
        let right = if self.current() == Kind::Flatten {
            Node::Identity
        } else {
            self.projection_rhs(Kind::Filter.binding_power())?
        };
        Ok(Node::FilterProjection(Box::new([left, right, condition])))
    }

    fn comparator(&mut self, kind: Kind, left: Node<R::Value>) -> Parsed<R::Value> {
        let comparator = match kind {
            Kind::Eq => Comparator::Eq,
            Kind::Ne => Comparator::Ne,
            Kind::Gt => Comparator::Gt,
            Kind::Gte => Comparator::Gte,
            Kind::Lt => Comparator::Lt,
            _ => Comparator::Lte,
        };
        let right = self.expression(5)?;
        Ok(Node::Comparator(
            comparator,
            Box::new(left),
            Box::new(right),
        ))
    }

    fn multi_select_list(&mut self) -> Parsed<R::Value> {
        let mut expressions = Vec::new();
        loop {
            expressions.push(self.expression(0)?);
            if self.current() == Kind::Rbracket {
                break;
            }
            self.matches(Kind::Comma)?;
        }
        self.matches(Kind::Rbracket)?;
        Ok(Node::MultiSelectList(expressions))
    }

    fn multi_select_dict(&mut self) -> Parsed<R::Value> {
        let mut pairs = Vec::new();
        loop {
            if !matches!(
                self.current(),
                Kind::QuotedIdentifier | Kind::UnquotedIdentifier
            ) {
                return Err(self.expected("['quoted_identifier', 'unquoted_identifier']"));
            }
            let key = name(&self.token(0).value);
            self.advance();
            self.matches(Kind::Colon)?;
            pairs.push((key, self.expression(0)?));
            match self.current() {
                Kind::Comma => self.matches(Kind::Comma)?,
                Kind::Rbrace => {
                    self.matches(Kind::Rbrace)?;
                    break;
                }
                _ => {}
            }
        }
        Ok(Node::MultiSelectDict(pairs))
    }

    fn projection_rhs(&mut self, binding_power: u8) -> Parsed<R::Value> {
        match self.current() {
            kind if kind.binding_power() < PROJECTION_STOP => Ok(Node::Identity),
            Kind::Lbracket | Kind::Filter => self.expression(binding_power),
            Kind::Dot => {
                self.matches(Kind::Dot)?;
                self.dot_rhs(binding_power)
            }
            _ => Err(self.error_at(self.token(0), "syntax error")),
        }
    }

    fn dot_rhs(&mut self, binding_power: u8) -> Parsed<R::Value> {
        match self.current() {
            Kind::QuotedIdentifier | Kind::UnquotedIdentifier | Kind::Star => {
                self.expression(binding_power)
            }
            Kind::Lbracket => {
                self.matches(Kind::Lbracket)?;
                self.multi_select_list()
            }
            Kind::Lbrace => {
                self.matches(Kind::Lbrace)?;
                self.multi_select_dict()
            }
            kind => {
                let reason = format!(
                    "Expecting: ['quoted_identifier', 'unquoted_identifier', \
                     'lbracket', 'lbrace'], got: {}",
                    kind.name()
                );
                Err(self.error_at(self.token(0), &reason))
            }
        }
    }
}

fn name<V>(value: &Value<V>) -> Box<[u8]> {
    match value {
        Value::Text(text) => text.clone().into(),
        _ => Box::default(),
    }
}
