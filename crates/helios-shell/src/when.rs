//! `when` expressions: where a command applies or a key binds, over named
//! context keys, as VS Code's `when` clauses.
//!
//! Grammar: `key` (true when the key is set and not false or empty),
//! `key == 'text'`, `key != 'text'`, `!expr`, `a && b`, `a || b`,
//! parentheses. `&&` binds tighter than `||`.

use std::collections::BTreeMap;

/// The current context: focus, view, selection... as named values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context(BTreeMap<String, String>);

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a boolean key.
    pub fn flag(mut self, key: &str, on: bool) -> Self {
        self.set(key, if on { "true" } else { "false" });
        self
    }

    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.set(key, value);
        self
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.0.insert(key.into(), value.into());
    }

    fn truthy(&self, key: &str) -> bool {
        self.0
            .get(key)
            .is_some_and(|v| !v.is_empty() && v != "false")
    }

    fn get(&self, key: &str) -> &str {
        self.0.get(key).map_or("", String::as_str)
    }
}

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub enum When {
    Always,
    Key(String),
    Eq(String, String),
    Ne(String, String),
    Not(Box<When>),
    And(Box<When>, Box<When>),
    Or(Box<When>, Box<When>),
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Text(String),
    Not,
    And,
    Or,
    Eq,
    Ne,
    Open,
    Close,
}

fn tokens(src: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '(' => {
                chars.next();
                out.push(Token::Open);
            }
            ')' => {
                chars.next();
                out.push(Token::Close);
            }
            '!' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(Token::Ne);
                } else {
                    out.push(Token::Not);
                }
            }
            '=' | '&' | '|' => {
                chars.next();
                if chars.next() != Some(c) {
                    return Err(format!("`{c}` must be doubled in `{src}`"));
                }
                out.push(match c {
                    '=' => Token::Eq,
                    '&' => Token::And,
                    _ => Token::Or,
                });
            }
            '\'' | '"' => {
                chars.next();
                let mut text = String::new();
                loop {
                    match chars.next() {
                        Some(q) if q == c => break,
                        Some(ch) => text.push(ch),
                        None => return Err(format!("unclosed text in `{src}`")),
                    }
                }
                out.push(Token::Text(text));
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' => {
                let mut ident = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' || c == '.' {
                        ident.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push(Token::Ident(ident));
            }
            other => return Err(format!("unexpected `{other}` in `{src}`")),
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn or(&mut self) -> Result<When, String> {
        let mut left = self.and()?;
        while self.peek() == Some(&Token::Or) {
            self.next();
            left = When::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<When, String> {
        let mut left = self.unary()?;
        while self.peek() == Some(&Token::And) {
            self.next();
            left = When::And(Box::new(left), Box::new(self.unary()?));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<When, String> {
        match self.next() {
            Some(Token::Not) => Ok(When::Not(Box::new(self.unary()?))),
            Some(Token::Open) => {
                let inner = self.or()?;
                match self.next() {
                    Some(Token::Close) => Ok(inner),
                    _ => Err("missing `)`".into()),
                }
            }
            Some(Token::Ident(key)) => match self.peek() {
                Some(Token::Eq | Token::Ne) => {
                    let eq = self.next() == Some(Token::Eq);
                    match self.next() {
                        Some(Token::Text(t) | Token::Ident(t)) => Ok(if eq {
                            When::Eq(key, t)
                        } else {
                            When::Ne(key, t)
                        }),
                        _ => Err(format!("`{key}` compared to nothing")),
                    }
                }
                _ => Ok(When::Key(key)),
            },
            other => Err(format!("unexpected {other:?}")),
        }
    }
}

impl When {
    /// Parses an expression; an empty one always applies.
    pub fn parse(src: &str) -> Result<When, String> {
        let tokens = tokens(src)?;
        if tokens.is_empty() {
            return Ok(When::Always);
        }
        let mut p = Parser { tokens, at: 0 };
        let w = p.or()?;
        if p.at != p.tokens.len() {
            return Err(format!("trailing tokens in `{src}`"));
        }
        Ok(w)
    }

    pub fn eval(&self, cx: &Context) -> bool {
        match self {
            When::Always => true,
            When::Key(k) => cx.truthy(k),
            When::Eq(k, v) => cx.get(k) == v,
            When::Ne(k, v) => cx.get(k) != v,
            When::Not(w) => !w.eval(cx),
            When::And(a, b) => a.eval(cx) && b.eval(cx),
            When::Or(a, b) => a.eval(cx) || b.eval(cx),
        }
    }

    /// How specific the expression is: the number of keys it tests. Among
    /// bindings of one layer, the most specific wins.
    pub fn specificity(&self) -> usize {
        match self {
            When::Always => 0,
            When::Key(_) | When::Eq(..) | When::Ne(..) => 1,
            When::Not(w) => w.specificity(),
            When::And(a, b) | When::Or(a, b) => a.specificity() + b.specificity(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_evaluate_against_the_context() {
        let cx = Context::new()
            .flag("graphFocus", true)
            .flag("textInputFocus", false)
            .with("view", "darkroom");
        for (src, expected) in [
            ("", true),
            ("graphFocus", true),
            ("textInputFocus", false),
            ("!textInputFocus", true),
            ("graphFocus && !textInputFocus", true),
            ("view == 'darkroom'", true),
            ("view != darkroom", false),
            ("viewportFocus || view == 'darkroom'", true),
            (
                "(viewportFocus || graphFocus) && view == \"lighttable\"",
                false,
            ),
            ("missing", false),
        ] {
            assert_eq!(When::parse(src).unwrap().eval(&cx), expected, "{src}");
        }
    }

    #[test]
    fn mistakes_are_named() {
        for bad in ["a & b", "(a", "a ==", "a b", "'open"] {
            assert!(When::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(When::parse("a && !b || c").unwrap().specificity(), 3);
    }
}
