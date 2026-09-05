//! A minimal, real SQL `SELECT` tokenizer and recursive-descent parser —
//! client-side only (`SQL-FR-001`, ADR-0034,
//! `docs/design/SERVER-SQL-SELECT-DESIGN.md`; `AGG-FR-001`, ADR-0035,
//! `docs/design/SERVER-SQL-AGGREGATE-DESIGN.md`). Produces a
//! domain-agnostic [`ParsedQuery`]: every name here is a plain
//! [`String`], not yet resolved to a [`super::protocol::FieldRef`] —
//! [`super::client::SchemaDrivenClient::query`] does that resolution
//! against the schema it already fetched (`SQL-FR-002`). The server
//! never sees SQL text at all; this module is used only by the client.
//!
//! Grammar (keywords case-insensitive; column/table/condition names are
//! plain identifiers, never quoted):
//!
//! ```text
//! query         := "SELECT" columns "FROM" table_ref [join_clause] [where_clause] [group_by_clause] [limit_clause]
//! table_ref     := ident [["AS"] ident]                      -- optional alias (JOIN-FR-005)
//! join_clause   := "JOIN" table_ref "ON" ident               -- ident names a declared relation
//! columns       := "*" | column_item ("," column_item)*
//! column_item   := qualified | ident | agg_call
//! qualified     := ident "." ident                           -- alias.field
//! agg_call      := agg_fn "(" ( "*" | ident ) ")"
//! agg_fn        := "COUNT" | "SUM" | "AVG" | "MIN" | "MAX"
//! where_clause  := "WHERE" condition ("AND" condition)*
//! condition     := (qualified | ident) comparator literal
//! comparator    := "=" | "!=" | "<" | "<=" | ">" | ">="
//! literal       := number | "'" ... "'" | "true" | "false"
//! group_by_clause := "GROUP" "BY" ident ("," ident)*
//! limit_clause  := "LIMIT" number
//! ```
//!
//! # `JOIN` (`JOIN-FR-005`, ADR-0044, protocol 12)
//!
//! `JOIN`'s `ON` names a *declared relation* (`neighbors`, a symmetric
//! label such as `relates_to`, `parent`, `children`), never a column
//! predicate — see `docs/design/SERVER-SQL-JOIN-DESIGN.md`. Three rules
//! are enforced here, at parse time, so they are syntax errors and never
//! a round trip: a `JOIN` query may not carry `GROUP BY` or an aggregate
//! call (`JoinWithAggregate`); in a `JOIN` query every column and every
//! condition must be qualified (`UnqualifiedInJoin`), because both sides
//! have the same field names and this parser does no ambiguity
//! resolution; and a qualifier must be one of the two sides' aliases (or
//! table names, when unaliased) — `UnknownQualifier` — with the two sides
//! distinguishable (`AmbiguousQualifiers` for `FROM entity JOIN entity`
//! with no aliases). Without `JOIN`, a qualified name whose qualifier is
//! the `FROM` alias or table name resolves exactly as the bare name would.
//! The right table name is *not* checked against the left here — the
//! client refuses a different one until ADR-0045 lands.
//!
//! `*` is only ever valid as `COUNT`'s own argument (`COUNT(*)`); every
//! other `agg_fn` requires a plain field `ident` — `SUM(*)` and
//! `COUNT(age)` are both syntax errors here, not silently accepted then
//! rejected later (`AGG-FR-008`'s own "`COUNT` is `COUNT(*)`-only"
//! non-goal). The `FROM` identifier is required by the grammar but never
//! looked up against anything — a connection serves exactly one domain,
//! so there is nothing to validate it against (a deliberate
//! simplification, named in the design rather than silently assumed). No
//! `OR`, no `HAVING`, no `LIKE`/`IN`/`IS NULL`/`BETWEEN`, no `ORDER BY`,
//! no nested/composite aggregate expressions, no subqueries, no
//! `INSERT`/`UPDATE`/`DELETE` — see each design document's own
//! "Non-goals".

use super::protocol::{AggregateFn, CompareOp};
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
    /// `alias.field`'s alias — `None` for a bare `field`. Validated
    /// against the query's table refs by [`parse`] (`JOIN-FR-005`).
    pub qualifier: Option<String>,
    pub name: String,
    pub op: CompareOp,
    pub value: Literal,
}

/// One `SELECT`-list item, before any name is resolved to a
/// [`super::protocol::FieldRef`] — `AGG-FR-001`, ADR-0035.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParsedColumnItem {
    Plain(String),
    /// `alias.field` — `JOIN-FR-005`, ADR-0044. In a `JOIN` query every
    /// plain column takes this form; without `JOIN` the qualifier must
    /// be the `FROM` alias/table and the item means the same as `Plain`.
    Qualified {
        qualifier: String,
        name: String,
    },
    Aggregate {
        func: AggregateFn,
        arg: AggregateArg,
    },
}

/// One `agg_call`'s argument — `Star` is valid only for
/// [`AggregateFn::Count`] (`COUNT(*)`); every other function requires
/// [`AggregateArg::Field`]. Enforced by the parser itself, not left for
/// a later resolution step (`AGG-FR-008`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AggregateArg {
    Star,
    Field(String),
}

/// The parsed `SELECT` column list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParsedColumns {
    All,
    Named(Vec<ParsedColumnItem>),
}

/// The full parsed shape of one `SELECT` string.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedQuery {
    pub columns: ParsedColumns,
    /// The `FROM` identifier — never validated against a server (a
    /// connection serves exactly one domain); since `JOIN-FR-005` it is
    /// compared against a `JOIN`'s table by the client.
    pub table: String,
    /// The `FROM` table's alias, if written (`JOIN-FR-005`).
    pub alias: Option<String>,
    /// The `JOIN` clause, if written (`JOIN-FR-005`, ADR-0044).
    pub join: Option<ParsedJoin>,
    pub conditions: Vec<ParsedCondition>,
    /// `GROUP BY`'s field list — empty when the clause was omitted.
    /// `AGG-FR-001`, ADR-0035.
    pub group_by: Vec<String>,
    pub limit: Option<usize>,
}

/// A parsed `JOIN table_ref ON relation` clause — `JOIN-FR-005`,
/// ADR-0044. `relation` is a declared relation's *name*, resolved by the
/// client against `Request::DescribeRelations`, never a column predicate.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedJoin {
    pub table: String,
    pub alias: Option<String>,
    pub relation: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SqlParseError {
    Expected {
        what: String,
        found: String,
    },
    UnexpectedEnd,
    InvalidNumber(String),
    NegativeLimit(i64),
    TrailingInput(String),
    UnterminatedString,
    UnexpectedCharacter(char),
    /// `agg_fn(...)` where `agg_fn` isn't one of `COUNT`/`SUM`/`AVG`/
    /// `MIN`/`MAX` (case-insensitively). `AGG-FR-001`, ADR-0035.
    UnknownAggregateFunction(String),
    /// `COUNT` given a field argument instead of `*` — this schema has
    /// no `NULL` concept, so `COUNT(field)` would be unconditionally
    /// identical to `COUNT(*)` (`AGG-FR-008`'s own non-goal).
    CountRequiresStar,
    /// `SUM`/`AVG`/`MIN`/`MAX` given `*` instead of a field — only
    /// `COUNT` accepts `*`.
    AggregateRequiresField(String),
    /// A `JOIN` query with `GROUP BY` or an aggregate call — aggregation
    /// over a join is a Non-goal (`JOIN-FR-005`, ADR-0044).
    JoinWithAggregate,
    /// A bare column or condition name in a `JOIN` query — both sides
    /// share field names, so every name must be `alias.field`.
    UnqualifiedInJoin(String),
    /// `alias.field` whose alias is neither side's alias or table name.
    UnknownQualifier(String),
    /// `FROM t JOIN t` with no aliases (or the same alias twice) — the
    /// two sides cannot be told apart.
    AmbiguousQualifiers(String),
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
            SqlParseError::UnknownAggregateFunction(name) => {
                write!(
                    f,
                    "{name:?} is not a known aggregate function (COUNT, SUM, AVG, MIN, MAX)"
                )
            }
            SqlParseError::CountRequiresStar => {
                write!(f, "COUNT requires * as its argument, e.g. COUNT(*)")
            }
            SqlParseError::AggregateRequiresField(func) => {
                write!(f, "{func} requires a field name as its argument, not *")
            }
            SqlParseError::JoinWithAggregate => {
                write!(
                    f,
                    "GROUP BY and aggregate functions are not supported with JOIN"
                )
            }
            SqlParseError::UnqualifiedInJoin(name) => {
                write!(
                    f,
                    "{name:?} must be qualified (alias.field) in a JOIN query"
                )
            }
            SqlParseError::UnknownQualifier(alias) => {
                write!(f, "{alias:?} is not an alias or table named in FROM/JOIN")
            }
            SqlParseError::AmbiguousQualifiers(name) => {
                write!(
                    f,
                    "{name:?} names both sides of the JOIN; give each side a distinct alias"
                )
            }
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
    /// `AGG-FR-001`, ADR-0035 — an aggregate call's argument list, e.g.
    /// `COUNT(*)`/`SUM(field)`. No other grammar production uses
    /// parentheses (`WHERE` stays a flat `AND`-list, unparenthesized).
    LParen,
    RParen,
    /// `JOIN-FR-005`, ADR-0044 — `alias.field`. The only use of `.`;
    /// numbers are integers here, so there is no fractional-literal
    /// ambiguity.
    Dot,
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
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::Dot => write!(f, "."),
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
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            '.' => {
                tokens.push(Token::Dot);
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
        let (qualifier, name) = self.maybe_qualified()?;
        let op = self.comparator()?;
        let value = self.literal()?;
        Ok(ParsedCondition {
            qualifier,
            name,
            op,
            value,
        })
    }

    /// `ident` or `ident "." ident` — `JOIN-FR-005`.
    fn maybe_qualified(&mut self) -> Result<(Option<String>, String), SqlParseError> {
        let first = self.ident()?;
        if matches!(self.peek(), Some(Token::Dot)) {
            self.advance();
            let name = self.ident()?;
            Ok((Some(first), name))
        } else {
            Ok((None, first))
        }
    }

    /// `table_ref := ident [["AS"] ident]` — `JOIN-FR-005`. An alias is
    /// any identifier that is not one of this grammar's own keywords, so
    /// `FROM dog WHERE …` still parses `WHERE` as the clause it is.
    fn table_ref(&mut self) -> Result<(String, Option<String>), SqlParseError> {
        let table = self.ident()?;
        if self.peek_keyword("AS") {
            self.advance();
            return Ok((table, Some(self.ident()?)));
        }
        match self.peek() {
            Some(Token::Ident(s)) if !is_keyword(s) => {
                let alias = s.clone();
                self.advance();
                Ok((table, Some(alias)))
            }
            _ => Ok((table, None)),
        }
    }

    fn columns(&mut self) -> Result<ParsedColumns, SqlParseError> {
        if matches!(self.peek(), Some(Token::Star)) {
            self.advance();
            return Ok(ParsedColumns::All);
        }
        let mut items = vec![self.column_item()?];
        while matches!(self.peek(), Some(Token::Comma)) {
            self.advance();
            items.push(self.column_item()?);
        }
        Ok(ParsedColumns::Named(items))
    }

    /// `AGG-FR-001`, ADR-0035: `ident`, or `agg_fn "(" ("*" | ident) ")"`.
    /// An aggregate call's argument shape is enforced right here, not
    /// left for a later resolution step — `COUNT(*)` is the only valid
    /// `Star` argument; every other function needs a plain field.
    fn column_item(&mut self) -> Result<ParsedColumnItem, SqlParseError> {
        let name = self.ident()?;
        if matches!(self.peek(), Some(Token::Dot)) {
            self.advance();
            let field = self.ident()?;
            return Ok(ParsedColumnItem::Qualified {
                qualifier: name,
                name: field,
            });
        }
        if !matches!(self.peek(), Some(Token::LParen)) {
            return Ok(ParsedColumnItem::Plain(name));
        }
        self.advance();
        let func = agg_fn(&name)?;
        let arg = if matches!(self.peek(), Some(Token::Star)) {
            self.advance();
            AggregateArg::Star
        } else {
            AggregateArg::Field(self.ident()?)
        };
        self.expect_rparen()?;
        match (func, &arg) {
            (AggregateFn::Count, AggregateArg::Star) => {}
            (AggregateFn::Count, AggregateArg::Field(_)) => {
                return Err(SqlParseError::CountRequiresStar)
            }
            (_, AggregateArg::Star) => return Err(SqlParseError::AggregateRequiresField(name)),
            (_, AggregateArg::Field(_)) => {}
        }
        Ok(ParsedColumnItem::Aggregate { func, arg })
    }

    fn expect_rparen(&mut self) -> Result<(), SqlParseError> {
        match self.advance() {
            Some(Token::RParen) => Ok(()),
            Some(other) => Err(SqlParseError::Expected {
                what: "')'".into(),
                found: other.to_string(),
            }),
            None => Err(SqlParseError::UnexpectedEnd),
        }
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

/// The grammar's own keywords — what a `table_ref` alias may not be
/// (`JOIN-FR-005`), so `FROM dog WHERE` never reads `WHERE` as an alias.
fn is_keyword(ident: &str) -> bool {
    [
        "SELECT", "FROM", "WHERE", "AND", "GROUP", "BY", "LIMIT", "JOIN", "ON", "AS",
    ]
    .iter()
    .any(|k| ident.eq_ignore_ascii_case(k))
}

/// Maps a `column_item`'s call-site identifier to the aggregate function
/// it names, case-insensitively — `AGG-FR-001`, ADR-0035.
fn agg_fn(name: &str) -> Result<AggregateFn, SqlParseError> {
    match name.to_ascii_uppercase().as_str() {
        "COUNT" => Ok(AggregateFn::Count),
        "SUM" => Ok(AggregateFn::Sum),
        "AVG" => Ok(AggregateFn::Avg),
        "MIN" => Ok(AggregateFn::Min),
        "MAX" => Ok(AggregateFn::Max),
        _ => Err(SqlParseError::UnknownAggregateFunction(name.to_string())),
    }
}

/// Parse one `SELECT` string end to end — `SQL-FR-001`, `AGG-FR-001`.
/// Never touches the network; a syntax error is reported entirely
/// client-side.
pub(crate) fn parse(sql: &str) -> Result<ParsedQuery, SqlParseError> {
    let tokens = tokenize(sql)?;
    let mut parser = Parser::new(&tokens);

    parser.expect_keyword("SELECT")?;
    let columns = parser.columns()?;
    parser.expect_keyword("FROM")?;
    let (table, alias) = parser.table_ref()?;

    let mut join = None;
    if parser.peek_keyword("JOIN") {
        parser.advance();
        let (right_table, right_alias) = parser.table_ref()?;
        parser.expect_keyword("ON")?;
        let relation = parser.ident()?;
        join = Some(ParsedJoin {
            table: right_table,
            alias: right_alias,
            relation,
        });
    }

    let mut conditions = Vec::new();
    if parser.peek_keyword("WHERE") {
        parser.advance();
        conditions.push(parser.condition()?);
        while parser.peek_keyword("AND") {
            parser.advance();
            conditions.push(parser.condition()?);
        }
    }

    let mut group_by = Vec::new();
    if parser.peek_keyword("GROUP") {
        parser.advance();
        parser.expect_keyword("BY")?;
        group_by.push(parser.ident()?);
        while matches!(parser.peek(), Some(Token::Comma)) {
            parser.advance();
            group_by.push(parser.ident()?);
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

    let query = ParsedQuery {
        columns,
        table,
        alias,
        join,
        conditions,
        group_by,
        limit,
    };
    validate_qualifiers(&query)?;
    Ok(query)
}

/// `JOIN-FR-005`'s three parse-time rules (see the module doc's own
/// "`JOIN`" section): no aggregation with `JOIN`; every name qualified
/// in a `JOIN` query; every qualifier one of the sides, and the two
/// sides distinguishable. Without `JOIN`, a qualifier must be the `FROM`
/// alias or table name.
fn validate_qualifiers(query: &ParsedQuery) -> Result<(), SqlParseError> {
    let left_names: Vec<&str> = std::iter::once(query.table.as_str())
        .chain(query.alias.as_deref())
        .collect();
    let right_names: Vec<&str> = match &query.join {
        Some(join) => std::iter::once(join.table.as_str())
            .chain(join.alias.as_deref())
            .collect(),
        None => Vec::new(),
    };
    let check = |qualifier: &str| -> Result<(), SqlParseError> {
        let is_left = left_names.iter().any(|n| n.eq_ignore_ascii_case(qualifier));
        let is_right = right_names
            .iter()
            .any(|n| n.eq_ignore_ascii_case(qualifier));
        match (is_left, is_right) {
            (true, true) => Err(SqlParseError::AmbiguousQualifiers(qualifier.to_string())),
            (false, false) => Err(SqlParseError::UnknownQualifier(qualifier.to_string())),
            _ => Ok(()),
        }
    };
    if let Some(join) = &query.join {
        let has_aggregate = matches!(&query.columns, ParsedColumns::Named(items)
            if items.iter().any(|i| matches!(i, ParsedColumnItem::Aggregate { .. })));
        if has_aggregate || !query.group_by.is_empty() {
            return Err(SqlParseError::JoinWithAggregate);
        }
        // Both sides must be tellable apart *before* any name is checked,
        // so `FROM entity JOIN entity ON r` is refused even for `SELECT *`.
        let left_alias = query.alias.as_deref().unwrap_or(&query.table);
        let right_alias = join.alias.as_deref().unwrap_or(&join.table);
        if left_alias.eq_ignore_ascii_case(right_alias) {
            return Err(SqlParseError::AmbiguousQualifiers(left_alias.to_string()));
        }
    }
    if let ParsedColumns::Named(items) = &query.columns {
        for item in items {
            match item {
                ParsedColumnItem::Plain(name) if query.join.is_some() => {
                    return Err(SqlParseError::UnqualifiedInJoin(name.clone()));
                }
                ParsedColumnItem::Qualified { qualifier, .. } => check(qualifier)?,
                _ => {}
            }
        }
    }
    for condition in &query.conditions {
        match &condition.qualifier {
            None if query.join.is_some() => {
                return Err(SqlParseError::UnqualifiedInJoin(condition.name.clone()));
            }
            Some(qualifier) => check(qualifier)?,
            None => {}
        }
    }
    Ok(())
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
            ParsedColumns::Named(vec![
                ParsedColumnItem::Plain("age".into()),
                ParsedColumnItem::Plain("breed".into()),
            ])
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

    // `AGG-FR-001`, ADR-0035.

    #[test]
    fn parses_count_star() {
        let q = parse("SELECT COUNT(*) FROM dog").unwrap();
        assert_eq!(
            q.columns,
            ParsedColumns::Named(vec![ParsedColumnItem::Aggregate {
                func: AggregateFn::Count,
                arg: AggregateArg::Star,
            }])
        );
        assert!(q.group_by.is_empty());
    }

    #[test]
    fn parses_every_aggregate_function_with_a_field_argument() {
        for (text, func) in [
            ("SUM", AggregateFn::Sum),
            ("AVG", AggregateFn::Avg),
            ("MIN", AggregateFn::Min),
            ("MAX", AggregateFn::Max),
        ] {
            let sql = format!("SELECT {text}(age) FROM dog");
            let q = parse(&sql).unwrap();
            assert_eq!(
                q.columns,
                ParsedColumns::Named(vec![ParsedColumnItem::Aggregate {
                    func,
                    arg: AggregateArg::Field("age".into()),
                }]),
                "for {text}"
            );
        }
    }

    #[test]
    fn parses_a_mixed_plain_and_aggregate_column_list() {
        let q = parse("SELECT breed, COUNT(*), SUM(age) FROM dog GROUP BY breed").unwrap();
        assert_eq!(
            q.columns,
            ParsedColumns::Named(vec![
                ParsedColumnItem::Plain("breed".into()),
                ParsedColumnItem::Aggregate {
                    func: AggregateFn::Count,
                    arg: AggregateArg::Star,
                },
                ParsedColumnItem::Aggregate {
                    func: AggregateFn::Sum,
                    arg: AggregateArg::Field("age".into()),
                },
            ])
        );
        assert_eq!(q.group_by, vec!["breed".to_string()]);
    }

    #[test]
    fn parses_group_by_with_one_and_with_several_fields() {
        let q = parse("SELECT breed, COUNT(*) FROM dog GROUP BY breed").unwrap();
        assert_eq!(q.group_by, vec!["breed".to_string()]);

        let q = parse("SELECT breed, age, COUNT(*) FROM dog GROUP BY breed, age").unwrap();
        assert_eq!(q.group_by, vec!["breed".to_string(), "age".to_string()]);
    }

    #[test]
    fn group_by_composes_with_where_and_limit() {
        let q =
            parse("SELECT breed, COUNT(*) FROM dog WHERE age > 1 GROUP BY breed LIMIT 5").unwrap();
        assert_eq!(q.conditions.len(), 1);
        assert_eq!(q.group_by, vec!["breed".to_string()]);
        assert_eq!(q.limit, Some(5));
    }

    #[test]
    fn an_unknown_aggregate_function_is_a_syntax_error() {
        assert_eq!(
            parse("SELECT NOPE(age) FROM dog"),
            Err(SqlParseError::UnknownAggregateFunction("NOPE".into()))
        );
    }

    #[test]
    fn count_given_a_field_instead_of_star_is_a_syntax_error() {
        assert_eq!(
            parse("SELECT COUNT(age) FROM dog"),
            Err(SqlParseError::CountRequiresStar)
        );
    }

    #[test]
    fn sum_avg_min_max_given_a_bare_star_are_syntax_errors() {
        for text in ["SUM", "AVG", "MIN", "MAX"] {
            let sql = format!("SELECT {text}(*) FROM dog");
            assert_eq!(
                parse(&sql),
                Err(SqlParseError::AggregateRequiresField(text.into())),
                "for {text}"
            );
        }
    }

    #[test]
    fn a_missing_by_after_group_is_a_syntax_error() {
        assert!(matches!(
            parse("SELECT breed, COUNT(*) FROM dog GROUP breed"),
            Err(SqlParseError::Expected { .. })
        ));
    }

    #[test]
    fn an_aggregate_call_with_no_closing_paren_is_a_syntax_error() {
        assert!(matches!(
            parse("SELECT COUNT(* FROM dog"),
            Err(SqlParseError::Expected { .. })
        ));
    }

    // `JOIN-FR-005` (ADR-0044): aliases, qualified names, `JOIN … ON`,
    // and every parse-time rejection.

    #[test]
    fn parses_a_from_alias_with_and_without_as() {
        let q = parse("SELECT d.age FROM dog d WHERE d.age > 3").unwrap();
        assert_eq!(q.table, "dog");
        assert_eq!(q.alias.as_deref(), Some("d"));
        assert_eq!(
            q.columns,
            ParsedColumns::Named(vec![ParsedColumnItem::Qualified {
                qualifier: "d".into(),
                name: "age".into(),
            }])
        );
        assert_eq!(q.conditions[0].qualifier.as_deref(), Some("d"));
        assert_eq!(q.conditions[0].name, "age");
        let q = parse("SELECT age FROM dog AS d").unwrap();
        assert_eq!(q.alias.as_deref(), Some("d"));
        // A keyword after the table is a clause, never an alias.
        let q = parse("SELECT age FROM dog WHERE age = 1 LIMIT 2").unwrap();
        assert_eq!(q.alias, None);
        // The table name itself qualifies, alias or not.
        assert!(parse("SELECT dog.age FROM dog").is_ok());
    }

    #[test]
    fn parses_join_on_a_relation_with_qualified_columns_and_conditions() {
        let q = parse(
            "SELECT a.label, b.label FROM entity a JOIN entity b ON relates_to \
             WHERE a.kind = 'person' AND b.mention_count > 4 LIMIT 10",
        )
        .unwrap();
        let join = q.join.as_ref().expect("a JOIN clause");
        assert_eq!(join.table, "entity");
        assert_eq!(join.alias.as_deref(), Some("b"));
        assert_eq!(join.relation, "relates_to");
        assert_eq!(q.alias.as_deref(), Some("a"));
        assert_eq!(
            q.columns,
            ParsedColumns::Named(vec![
                ParsedColumnItem::Qualified {
                    qualifier: "a".into(),
                    name: "label".into(),
                },
                ParsedColumnItem::Qualified {
                    qualifier: "b".into(),
                    name: "label".into(),
                },
            ])
        );
        assert_eq!(q.conditions.len(), 2);
        assert_eq!(q.conditions[0].qualifier.as_deref(), Some("a"));
        assert_eq!(q.conditions[1].qualifier.as_deref(), Some("b"));
        assert_eq!(q.limit, Some(10));
        // `SELECT *` over a join, `AS` aliases, `parent`/`children` names.
        let q = parse("SELECT * FROM employee AS e JOIN employee AS m ON parent").unwrap();
        assert_eq!(q.columns, ParsedColumns::All);
        assert_eq!(q.join.unwrap().relation, "parent");
    }

    #[test]
    fn a_join_with_group_by_or_an_aggregate_is_a_syntax_error() {
        assert_eq!(
            parse("SELECT a.kind, COUNT(*) FROM entity a JOIN entity b ON neighbors GROUP BY kind"),
            Err(SqlParseError::JoinWithAggregate)
        );
        assert_eq!(
            parse("SELECT COUNT(*) FROM entity a JOIN entity b ON neighbors"),
            Err(SqlParseError::JoinWithAggregate)
        );
    }

    #[test]
    fn an_unqualified_name_in_a_join_query_is_a_syntax_error() {
        assert_eq!(
            parse("SELECT label FROM entity a JOIN entity b ON neighbors"),
            Err(SqlParseError::UnqualifiedInJoin("label".into()))
        );
        assert_eq!(
            parse("SELECT a.label FROM entity a JOIN entity b ON neighbors WHERE kind = 'x'"),
            Err(SqlParseError::UnqualifiedInJoin("kind".into()))
        );
    }

    #[test]
    fn an_unknown_or_ambiguous_qualifier_is_a_syntax_error() {
        assert_eq!(
            parse("SELECT c.label FROM entity a JOIN entity b ON neighbors"),
            Err(SqlParseError::UnknownQualifier("c".into()))
        );
        assert_eq!(
            parse("SELECT x.age FROM dog"),
            Err(SqlParseError::UnknownQualifier("x".into()))
        );
        // Same table, no aliases: the sides cannot be told apart.
        assert_eq!(
            parse("SELECT * FROM entity JOIN entity ON neighbors"),
            Err(SqlParseError::AmbiguousQualifiers("entity".into()))
        );
        // The table name qualifies a side only when it is unambiguous.
        assert_eq!(
            parse("SELECT entity.label FROM entity a JOIN entity b ON neighbors"),
            Err(SqlParseError::AmbiguousQualifiers("entity".into()))
        );
    }

    #[test]
    fn a_join_without_on_or_a_relation_is_a_syntax_error() {
        assert!(matches!(
            parse("SELECT a.label FROM entity a JOIN entity b"),
            Err(SqlParseError::UnexpectedEnd)
        ));
        assert!(matches!(
            parse("SELECT a.label FROM entity a JOIN entity b WHERE a.kind = 'x'"),
            Err(SqlParseError::Expected { .. })
        ));
    }
}
