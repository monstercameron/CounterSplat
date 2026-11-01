//! A small preprocessor for the PlayCanvas WGSL dialect.
//!
//! Supported: `#include "key"`, `#define NAME [VALUE]`, `#ifdef`, `#ifndef`, `#if EXPR`,
//! `#elif EXPR`, `#else`, `#endif`. Expressions support `defined(X)`, integers, identifiers
//! (undefined identifiers are 0), `! && || == != < <= > >=`, `+ -` and parentheses.
//! Unsupported directives are an error, so unknown syntax fails loudly.

use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub enum Error {
    UnknownInclude(String),
    UnknownDirective(String),
    UnbalancedConditional,
    BadExpression(String),
    /// `#include "name", ARG` style templated includes are expanded by the engine, not here.
    UnsupportedInclude(String),
}

pub type Defines = HashMap<String, String>;

/// Chunks the engine registers at runtime (for example the stream declarations built by
/// `declarations::wgsl_input_declarations`). These take priority over the extracted source files.
pub type Overrides = HashMap<String, String>;

pub fn preprocess(key: &str, defines: &mut Defines) -> Result<String, Error> {
    preprocess_with(key, defines, &Overrides::new())
}

pub fn preprocess_with(
    key: &str,
    defines: &mut Defines,
    overrides: &Overrides,
) -> Result<String, Error> {
    let src = resolve(key, overrides).ok_or_else(|| Error::UnknownInclude(key.to_string()))?;
    process_source(&src, defines, overrides, 0)
}

pub fn preprocess_source(src: &str, defines: &mut Defines) -> Result<String, Error> {
    process_source(src, defines, &Overrides::new(), 0)
}

fn resolve(key: &str, overrides: &Overrides) -> Option<String> {
    match overrides.get(key) {
        Some(s) => Some(s.clone()),
        None => crate::chunk_by_key(key).map(str::to_string),
    }
}

fn process_source(
    src: &str,
    defines: &mut Defines,
    overrides: &Overrides,
    depth: u32,
) -> Result<String, Error> {
    if depth > 32 {
        return Err(Error::UnknownInclude("include depth exceeded".into()));
    }
    // Each frame: (this branch is active, a branch has been taken already)
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let mut out = String::new();
    let active = |s: &Vec<(bool, bool)>| s.iter().all(|(a, _)| *a);
    for line in src.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix('#') {
            // Strip trailing // and /* */ comments from the directive line.
            let rest = rest.split("//").next().unwrap_or("");
            let rest = rest.split("/*").next().unwrap_or("").trim();
            let (dir, arg) = match rest.find(char::is_whitespace) {
                Some(i) => (&rest[..i], rest[i..].trim()),
                None => (rest, ""),
            };
            match dir {
                "ifdef" => {
                    let parent = active(&stack);
                    push_cond(&mut stack, parent, defines.contains_key(arg.trim()))
                }
                "ifndef" => {
                    let parent = active(&stack);
                    push_cond(&mut stack, parent, !defines.contains_key(arg.trim()))
                }
                "if" => {
                    let parent = active(&stack);
                    let v = if parent {
                        eval(arg, defines)? != 0
                    } else {
                        false
                    };
                    push_cond(&mut stack, parent, v)
                }
                "elif" => {
                    let parent = stack
                        .iter()
                        .take(stack.len().saturating_sub(1))
                        .all(|(a, _)| *a);
                    let top = stack.last_mut().ok_or(Error::UnbalancedConditional)?;
                    if top.1 || !parent {
                        top.0 = false;
                    } else {
                        let v = eval(arg, defines)? != 0;
                        top.0 = v;
                        top.1 = v;
                    }
                }
                "else" => {
                    let parent = stack
                        .iter()
                        .take(stack.len().saturating_sub(1))
                        .all(|(a, _)| *a);
                    let top = stack.last_mut().ok_or(Error::UnbalancedConditional)?;
                    top.0 = parent && !top.1;
                    top.1 = true;
                }
                "endif" => {
                    stack.pop().ok_or(Error::UnbalancedConditional)?;
                }
                _ if !active(&stack) => {}
                "define" => {
                    let mut parts = arg.splitn(2, char::is_whitespace);
                    let name = parts.next().unwrap_or("").to_string();
                    let value = parts.next().unwrap_or("").trim().to_string();
                    defines.insert(name, value);
                }
                "include" => {
                    let mut parts = arg.splitn(3, '"');
                    let (_, key, tail) = (
                        parts.next(),
                        parts.next().unwrap_or(""),
                        parts.next().unwrap_or(""),
                    );
                    if !tail.trim().is_empty() {
                        return Err(Error::UnsupportedInclude(arg.to_string()));
                    }
                    let body = resolve(key, overrides)
                        .ok_or_else(|| Error::UnknownInclude(key.to_string()))?;
                    out.push_str(&process_source(&body, defines, overrides, depth + 1)?);
                    out.push('\n');
                }
                // WGSL's own directives and comments are passed through untouched.
                _ if dir.is_empty() || rest.starts_with('!') => out.push_str(line),
                other => return Err(Error::UnknownDirective(other.to_string())),
            }
            continue;
        }
        if active(&stack) {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !stack.is_empty() {
        return Err(Error::UnbalancedConditional);
    }
    Ok(out)
}

fn push_cond(stack: &mut Vec<(bool, bool)>, parent_active: bool, cond: bool) {
    let v = parent_active && cond;
    stack.push((v, v));
}

// ---- expression evaluator -------------------------------------------------------------

fn eval(expr: &str, defines: &Defines) -> Result<i64, Error> {
    let toks = tokenize(expr)?;
    let mut p = Parser {
        toks,
        pos: 0,
        defines,
    };
    let v = p.or()?;
    if p.pos != p.toks.len() {
        return Err(Error::BadExpression(expr.to_string()));
    }
    Ok(v)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(i64),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, Error> {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let st = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            out.push(Tok::Num(
                b[st..i].iter().collect::<String>().parse().unwrap(),
            ));
        } else if c.is_alphabetic() || c == '_' {
            let st = i;
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(b[st..i].iter().collect()));
        } else if c == '(' {
            out.push(Tok::LParen);
            i += 1;
        } else if c == ')' {
            out.push(Tok::RParen);
            i += 1;
        } else {
            let two: String = b[i..(i + 2).min(b.len())].iter().collect();
            let op = ["&&", "||", "==", "!=", "<=", ">="]
                .iter()
                .find(|o| **o == two)
                .copied()
                .or_else(|| {
                    ["!", "<", ">", "+", "-"]
                        .iter()
                        .find(|o| o.starts_with(c) && o.len() == 1)
                        .copied()
                })
                .ok_or_else(|| Error::BadExpression(s.to_string()))?;
            out.push(Tok::Op(op));
            i += op.len();
        }
    }
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    defines: &'a Defines,
}

impl Parser<'_> {
    fn peek_op(&self) -> Option<&'static str> {
        match self.toks.get(self.pos) {
            Some(Tok::Op(o)) => Some(*o),
            _ => None,
        }
    }
    fn or(&mut self) -> Result<i64, Error> {
        let mut v = self.and()?;
        while self.peek_op() == Some("||") {
            self.pos += 1;
            let r = self.and()?;
            v = ((v != 0) || (r != 0)) as i64;
        }
        Ok(v)
    }
    fn and(&mut self) -> Result<i64, Error> {
        let mut v = self.cmp()?;
        while self.peek_op() == Some("&&") {
            self.pos += 1;
            let r = self.cmp()?;
            v = ((v != 0) && (r != 0)) as i64;
        }
        Ok(v)
    }
    fn cmp(&mut self) -> Result<i64, Error> {
        let mut v = self.add()?;
        while let Some(op) = self.peek_op() {
            if !matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=") {
                break;
            }
            self.pos += 1;
            let r = self.add()?;
            v = match op {
                "==" => v == r,
                "!=" => v != r,
                "<" => v < r,
                "<=" => v <= r,
                ">" => v > r,
                _ => v >= r,
            } as i64;
        }
        Ok(v)
    }
    fn add(&mut self) -> Result<i64, Error> {
        let mut v = self.unary()?;
        while let Some(op) = self.peek_op() {
            if op != "+" && op != "-" {
                break;
            }
            self.pos += 1;
            let r = self.unary()?;
            v = if op == "+" { v + r } else { v - r };
        }
        Ok(v)
    }
    fn unary(&mut self) -> Result<i64, Error> {
        if self.peek_op() == Some("!") {
            self.pos += 1;
            return Ok((self.unary()? == 0) as i64);
        }
        if self.peek_op() == Some("-") {
            self.pos += 1;
            return Ok(-self.unary()?);
        }
        self.atom()
    }
    fn atom(&mut self) -> Result<i64, Error> {
        let t = self
            .toks
            .get(self.pos)
            .cloned()
            .ok_or_else(|| Error::BadExpression("unexpected end".into()))?;
        self.pos += 1;
        match t {
            Tok::Num(n) => Ok(n),
            Tok::LParen => {
                let v = self.or()?;
                if self.toks.get(self.pos) != Some(&Tok::RParen) {
                    return Err(Error::BadExpression("missing )".into()));
                }
                self.pos += 1;
                Ok(v)
            }
            Tok::Ident(name) if name == "defined" => {
                let paren = self.toks.get(self.pos) == Some(&Tok::LParen);
                if paren {
                    self.pos += 1;
                }
                let Some(Tok::Ident(n)) = self.toks.get(self.pos).cloned() else {
                    return Err(Error::BadExpression("defined needs a name".into()));
                };
                self.pos += 1;
                if paren {
                    if self.toks.get(self.pos) != Some(&Tok::RParen) {
                        return Err(Error::BadExpression("missing )".into()));
                    }
                    self.pos += 1;
                }
                Ok(self.defines.contains_key(&n) as i64)
            }
            Tok::Ident(name) => match self.defines.get(&name) {
                None => Ok(0),
                Some(v) if v.is_empty() => Ok(1),
                Some(v) => match v.trim().parse::<i64>() {
                    Ok(n) => Ok(n),
                    // A define whose value is another expression, such as FOG = NONE.
                    Err(_) => eval(v, self.defines),
                },
            },
            _ => Err(Error::BadExpression("unexpected token".into())),
        }
    }
}
