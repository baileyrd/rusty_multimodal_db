//! A minimal, real SQL `SELECT` tokenizer and recursive-descent parser —
//! client-side only (`SQL-FR-001`, ADR-0034,
//! `docs/design/SERVER-SQL-SELECT-DESIGN.md`). Produces a domain-agnostic
//! [`ParsedQuery`]: every name here is a plain [`String`], not yet
//! resolved to a [`super::protocol::FieldRef`] — [`super::client::SchemaDrivenClient::query`]
//! does that resolution against the schema it already fetched
//! (`SQL-FR-002`). The server never sees SQL text at all; this module is
//! used only by the client.
//!
//! Grammar (keywords case-insensitive; column/table/condition names are
//! plain identifiers, never quoted):
//!
//! ```text
//! query         := "SELECT" columns "FROM" ident [where_clause] [limit_clause]
//! columns       := "*" | ident ("," ident)*
//! where_clause  := "WHERE" condition ("AND" condition)*
//! condition     := ident comparator literal
//! comparator    := "=" | "!=" | "<" | "<=" | ">" | ">="
//! literal       := number | "'" ... "'" | "true" | "false"
//! limit_clause  := "LIMIT" number
//! ```
//!
//! The `FROM` identifier is required by the grammar but never looked up
//! against anything — a connection serves exactly one domain, so there is
//! nothing to validate it against (a deliberate simplification, named in
//! the design rather than silently assumed). No `OR`, no parentheses, no
//! `LIKE`/`IN`/`IS NULL`/`BETWEEN`, no `ORDER BY`, no subqueries, no
//! `INSERT`/`UPDATE`/`DELETE` — see the design document's own "Non-goals".

use super::protocol::CompareOp;
use std::fmt;

/// One `WHERE`-clause literal, before it is resolved against a field's
/// real [`super::protocol::ValueKind`] — [`super::client::SchemaDrivenClient::query`]
/// does that resolution (`SQL-FR-002`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Literal {
    Number(i64),
    Str(String),
    Bool(bool),
}

/// One parsed `WHERE`-clause condition — `name` is not yet a
/// [`super::protocol::FieldRef`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedCondition {
    pub name: String,
    pub op: CompareOp,
    pub value: Literal,
}

/// The parsed `SELECT` column list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParsedColumns {
    All,
    Named(Vec<String>),
}

/// The full parsed shape of one `SELECT` string.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedQuery {
    pub columns: ParsedColumns,
    /// The `FROM` identifier — carried for completeness, never validated
    /// against anything (see this module's own doc comment).
    #[allow(dead_code)]
    pub table: String,
    pub conditions: Vec<ParsedCondition>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SqlParseError {
    Expected { what: String, found: String },
    UnexpectedEnd,
    InvalidNumber(String),
    NegativeLimit(i64),
    TrailingInput(String),
    UnterminatedString,
    UnexpectedCharacter(char),
}

impl fmt::Display for SqlParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlParseError::Expected { what, found } => {
                write!(f, "expected {what}, found {found}")
            }
            SqlParseError::UnexpectedEnd => write!(f, "unexpected end of query"),
            SqlParseError::InvalidNumber(text) => write!(f, "invalid number literal {text:?}"),
            SqlParseError::NegativeLimit(n) => write!(f, "LIMIT must not be negative, got {n}"),
            SqlParseError::TrailingInput(text) => {
                write!(f, "unexpected trailing input starting at {text}")
            }
            SqlParseError::UnterminatedString => write!(f, "unterminated string literal"),
            SqlParseError::UnexpectedCharacter(c) => write!(f, "unexpected character {c:?}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Number(i64),
    Str(String),
    Star,
    Comma,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Ident(s) => write!(f, "{s:?}"),
            Token::Number(n) => write!(f, "{n}"),
            Token::Str(s) => write!(f, "'{s}'"),
            Token::Star => write!(f, "*"),
            Token::Comma => write!(f, ","),
            Token::Eq => write!(f, "="),
            Token::Ne => write!(f, "!="),
            Token::Lt => write!(f, "<"),
            Token::Le => write!(f, "<="),
            Token::Gt => write!(f, ">"),
            Token::Ge => write!(f, ">="),
        }
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>, SqlParseError> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            '=' => {
                tokens.push(Token::Eq);
                i += 1;
            }
            '!' if chars.get(i + 1) == Some(&'=') => {
                tokens.push(Token::Ne);
                i += 2;
            }
            '<' if chars.get(i + 1) == Some(&'=') => {
                tokens.push(Token::Le);
                i += 2;
            }
            '<' => {
                tokens.push(Token::Lt);
                i += 1;
            }
            '>' if chars.get(i + 1) == Some(&'=') => {
                tokens.push(Token::Ge);
                i += 2;
            }
            '>' => {
                tokens.push(Token::Gt);
                i += 1;
            }
            '\'' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        Some('\'') => {
                            i += 1;
                            break;
                        }
                        Some(&ch) => {
                            s.push(ch);
                            i += 1;
                        }
                        None => return Err(SqlParseError::UnterminatedString),
                    }
                }
                tokens.push(Token::Str(s));
            }
            '-' if chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                let start = i;
                i += 1;
                while chars.get(i).is_some_and(char::is_ascii_digit) {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let n = text
                    .parse::<i64>()
                    .map_err(|_| SqlParseError::InvalidNumber(text.clone()))?;
                tokens.push(Token::Number(n));
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while chars.get(i).is_some_and(char::is_ascii_digit) {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let n = text
                    .parse::<i64>()
                    .map_err(|_| SqlParseError::InvalidNumber(text.clone()))?;
                tokens.push(Token::Number(n));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while chars
                    .get(i)
                    .is_some_and(|d| d.is_alphanumeric() || *d == '_')
                {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                tokens.push(Token::Ident(text));
            }
            other => return Err(SqlParseError::UnexpectedCharacter(other)),
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn peek_keyword(&self, keyword: &str) -> bool {
        matches!(self.peek(), Some(Token::Ident(s)) if s.eq_ignore_ascii_case(keyword))
    }

    fn expect_keyword(&mut self, keyword: &str) -> Result<(), SqlParseError> {
        match self.advance() {
            Some(Token::Ident(s)) if s.eq_ignore_ascii_case(keyword) => Ok(()),
            Some(other) => Err(SqlParseError::Expected {
                what: format!("{keyword:?}"),
                found: other.to_string(),
            }),
            None => Err(SqlParseError::UnexpectedEnd),
        }
    }

    fn ident(&mut self) -> Result<String, SqlParseError> {
        match self.advance() {
            Some(Token::Ident(s)) => Ok(s.clone()),
            Some(other) => Err(SqlParseError::Expected {
                what: "an identifier".into(),
                found: other.to_string(),
            }),
            None => Err(SqlParseError::UnexpectedEnd),
        }
    }

    fn comparator(&mut self) -> Result<CompareOp, SqlParseError> {
        match self.advance() {
            Some(Token::Eq) => Ok(CompareOp::Eq),
            Some(Token::Ne) => Ok(CompareOp::Ne),
            Some(Token::Lt) => Ok(CompareOp::Lt),
            Some(Token::Le) => Ok(CompareOp::Le),
            Some(Token::Gt) => Ok(CompareOp::Gt),
            Some(Token::Ge) => Ok(CompareOp::Ge),
            Some(other) => Err(SqlParseError::Expected {
                what: "a comparator (=, !=, <, <=, >, >=)".into(),
                found: other.to_string(),
            }),
            None => Err(SqlParseError::UnexpectedEnd),
        }
    }

    fn literal(&mut self) -> Result<Literal, SqlParseError> {
        match self.advance() {
            Some(Token::Number(n)) => Ok(Literal::Number(*n)),
            Some(Token::Str(s)) => Ok(Literal::Str(s.clone())),
            Some(Token::Ident(s)) if s.eq_ignore_ascii_case("true") => Ok(Literal::Bool(true)),
            Some(Token::Ident(s)) if s.eq_ignore_ascii_case("false") => Ok(Literal::Bool(false)),
            Some(other) => Err(SqlParseError::Expected {
                what: "a literal (a number, a 'string', true, or false)".into(),
                found: other.to_string(),
            }),
            None => Err(SqlParseError::UnexpectedEnd),
        }
    }

    fn condition(&mut self) -> Result<ParsedCondition, SqlParseError> {
        let name = self.ident()?;
        let op = self.comparator()?;
        let value = self.literal()?;
        Ok(ParsedCondition { name, op, value })
    }

    fn columns(&mut self) -> Result<ParsedColumns, SqlParseError> {
        if matches!(self.peek(), Some(Token::Star)) {
            self.advance();
            return Ok(ParsedColumns::All);
        }
        let mut names = vec![self.ident()?];
        while matches!(self.peek(), Some(Token::Comma)) {
            self.advance();
            names.push(self.ident()?);
        }
        Ok(ParsedColumns::Named(names))
    }

    fn limit_number(&mut self) -> Result<usize, SqlParseError> {
        match self.advance() {
            Some(Token::Number(n)) if *n >= 0 => Ok(*n as usize),
            Some(Token::Number(n)) => Err(SqlParseError::NegativeLimit(*n)),
            Some(other) => Err(SqlParseError::Expected {
                what: "a non-negative number".into(),
                found: other.to_string(),
            }),
            None => Err(SqlParseError::UnexpectedEnd),
        }
    }
}

/// Parse one `SELECT` string end to end — `SQL-FR-001`. Never touches
/// the network; a syntax error is reported entirely client-side.
pub(crate) fn parse(sql: &str) -> Result<ParsedQuery, SqlParseError> {
    let tokens = tokenize(sql)?;
    let mut parser = Parser::new(&tokens);

    parser.expect_keyword("SELECT")?;
    let columns = parser.columns()?;
    parser.expect_keyword("FROM")?;
    let table = parser.ident()?;

    let mut conditions = Vec::new();
    if parser.peek_keyword("WHERE") {
        parser.advance();
        conditions.push(parser.condition()?);
        while parser.peek_keyword("AND") {
            parser.advance();
            conditions.push(parser.condition()?);
        }
    }

    let mut limit = None;
    if parser.peek_keyword("LIMIT") {
        parser.advance();
        limit = Some(parser.limit_number()?);
    }

    if let Some(remaining) = parser.peek() {
        return Err(SqlParseError::TrailingInput(remaining.to_string()));
    }

    Ok(ParsedQuery {
        columns,
        table,
        conditions,
        limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_select_star_with_no_where_or_limit() {
        let q = parse("SELECT * FROM dog").unwrap();
        assert_eq!(q.columns, ParsedColumns::All);
        assert_eq!(q.table, "dog");
        assert!(q.conditions.is_empty());
        assert_eq!(q.limit, None);
    }

    #[test]
    fn parses_named_columns_case_insensitive_keywords() {
        let q = parse("select age, breed from dog").unwrap();
        assert_eq!(
            q.columns,
            ParsedColumns::Named(vec!["age".into(), "breed".into()])
        );
    }

    #[test]
    fn parses_every_comparator_and_and_conjunction() {
        let q = parse(
            "SELECT * FROM dog WHERE age > 3 AND age <= 10 AND age < 20 AND age >= 1 \
             AND age = 5 AND age != 6",
        )
        .unwrap();
        let ops: Vec<CompareOp> = q.conditions.iter().map(|c| c.op).collect();
        assert_eq!(
            ops,
            vec![
                CompareOp::Gt,
                CompareOp::Le,
                CompareOp::Lt,
                CompareOp::Ge,
                CompareOp::Eq,
                CompareOp::Ne,
            ]
        );
    }

    #[test]
    fn parses_string_true_false_and_negative_number_literals() {
        let q = parse(
            "SELECT * FROM dog WHERE breed = 'labrador' AND active = true \
             AND retired = false AND balance = -5",
        )
        .unwrap();
        assert_eq!(q.conditions[0].value, Literal::Str("labrador".into()));
        assert_eq!(q.conditions[1].value, Literal::Bool(true));
        assert_eq!(q.conditions[2].value, Literal::Bool(false));
        assert_eq!(q.conditions[3].value, Literal::Number(-5));
    }

    #[test]
    fn parses_limit() {
        let q = parse("SELECT * FROM dog LIMIT 10").unwrap();
        assert_eq!(q.limit, Some(10));
        let q = parse("SELECT * FROM dog WHERE age = 3 LIMIT 1").unwrap();
        assert_eq!(q.limit, Some(1));
    }

    #[test]
    fn missing_select_or_from_is_a_syntax_error() {
        assert!(matches!(
            parse("FROM dog"),
            Err(SqlParseError::Expected { .. })
        ));
        assert!(matches!(
            parse("SELECT * dog"),
            Err(SqlParseError::Expected { .. })
        ));
    }

    #[test]
    fn a_bad_literal_or_operator_is_a_syntax_error() {
        assert!(matches!(
            parse("SELECT * FROM dog WHERE age ~ 3"),
            Err(SqlParseError::UnexpectedCharacter('~'))
        ));
        assert!(matches!(
            parse("SELECT * FROM dog WHERE age = "),
            Err(SqlParseError::UnexpectedEnd)
        ));
    }

    #[test]
    fn an_unterminated_string_is_a_syntax_error() {
        assert_eq!(
            parse("SELECT * FROM dog WHERE breed = 'labrador"),
            Err(SqlParseError::UnterminatedString)
        );
    }

    #[test]
    fn a_negative_limit_is_rejected() {
        assert_eq!(
            parse("SELECT * FROM dog LIMIT -1"),
            Err(SqlParseError::NegativeLimit(-1))
        );
    }

    #[test]
    fn trailing_input_after_a_complete_query_is_rejected() {
        assert!(matches!(
            parse("SELECT * FROM dog LIMIT 1 extra"),
            Err(SqlParseError::TrailingInput(_))
        ));
    }

    #[test]
    fn or_and_parentheses_are_not_supported() {
        // `OR` parses as a bare identifier where a comparator is expected —
        // a syntax error, not silently accepted as a second condition.
        assert!(parse("SELECT * FROM dog WHERE age = 3 OR age = 4").is_err());
        assert!(parse("SELECT * FROM dog WHERE (age = 3)").is_err());
    }
}
