//! Source text -> tokens. Shared by both variants.
//!
//! Written for this experiment rather than reused from `tinyvm-qjs`: measured,
//! not assumed. See `RESULTS.md`, deviation D1 -- the upstream lexer compiles
//! into a foreign crate unchanged, but lexes `function`, `{`, `return`,
//! `while`, `=`, `==`, `<` and every string literal to `Unsupported`, so it
//! cannot read one line of the corpus section 2 requires.

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Kw(Kw),
    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semi,
    Assign,
    Plus,
    Minus,
    Star,
    Slash,
    EqEq,
    BangEq,
    Lt,
    Le,
    Gt,
    Ge,
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kw {
    Function,
    Let,
    If,
    Else,
    While,
    Return,
}

pub fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            let text = &src[start..i];
            let value: f64 = text
                .parse()
                .map_err(|_| format!("bad number literal `{text}` at byte {start}"))?;
            out.push(Tok::Num(value));
            continue;
        }
        if c == b'"' {
            i += 1;
            let mut s = String::new();
            loop {
                match b.get(i) {
                    None => return Err(format!("unterminated string at byte {i}")),
                    Some(b'"') => {
                        i += 1;
                        break;
                    }
                    Some(b'\\') => {
                        i += 1;
                        match b.get(i) {
                            Some(b'n') => s.push('\n'),
                            Some(b'\\') => s.push('\\'),
                            Some(b'"') => s.push('"'),
                            other => {
                                return Err(format!("unsupported escape {other:?} at byte {i}"));
                            }
                        }
                        i += 1;
                    }
                    Some(_) => {
                        let ch = src[i..].chars().next().expect("valid utf-8");
                        s.push(ch);
                        i += ch.len_utf8();
                    }
                }
            }
            out.push(Tok::Str(s));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            out.push(match word {
                "function" => Tok::Kw(Kw::Function),
                "let" => Tok::Kw(Kw::Let),
                "if" => Tok::Kw(Kw::If),
                "else" => Tok::Kw(Kw::Else),
                "while" => Tok::Kw(Kw::While),
                "return" => Tok::Kw(Kw::Return),
                _ => Tok::Ident(word.to_string()),
            });
            continue;
        }
        let two = src.get(i..i + 2);
        let (len, tok) = match (c, two) {
            (_, Some("==")) => (2, Tok::EqEq),
            (_, Some("!=")) => (2, Tok::BangEq),
            (_, Some("<=")) => (2, Tok::Le),
            (_, Some(">=")) => (2, Tok::Ge),
            (b'(', _) => (1, Tok::LParen),
            (b')', _) => (1, Tok::RParen),
            (b'{', _) => (1, Tok::LBrace),
            (b'}', _) => (1, Tok::RBrace),
            (b',', _) => (1, Tok::Comma),
            (b';', _) => (1, Tok::Semi),
            (b'=', _) => (1, Tok::Assign),
            (b'+', _) => (1, Tok::Plus),
            (b'-', _) => (1, Tok::Minus),
            (b'*', _) => (1, Tok::Star),
            (b'/', _) => (1, Tok::Slash),
            (b'<', _) => (1, Tok::Lt),
            (b'>', _) => (1, Tok::Gt),
            _ => return Err(format!("unexpected character `{}` at byte {i}", c as char)),
        };
        i += len;
        out.push(tok);
    }
    out.push(Tok::Eof);
    Ok(out)
}
