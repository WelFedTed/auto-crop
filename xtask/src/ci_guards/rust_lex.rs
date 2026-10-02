// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! A tiny Rust lexer, just enough for the policy guards: it separates code tokens from comments
//! and string, raw-string and character literals, so a guard never fires on the word `unsafe` in
//! a doc comment or a string, and never misses it behind one.
//!
//! It is not a parser. It tracks the line of every token, the comment text on every line, and
//! whether a line holds code at all.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    Punct(char),
    /// A string, raw string, byte string, character or number literal (content dropped).
    Lit,
}

#[derive(Debug, Default)]
pub struct Lexed {
    /// Code tokens with their 1-based line.
    pub toks: Vec<(Tok, usize)>,
    /// Comment text per line, index = line number (index 0 unused). Doc comments included.
    pub comments: Vec<String>,
    /// Whether a line holds at least one code token, index = line number.
    pub has_code: Vec<bool>,
    /// First code token of each line, index = line number.
    pub first_tok: Vec<Option<Tok>>,
}

impl Lexed {
    fn ensure(&mut self, line: usize) {
        while self.comments.len() <= line {
            self.comments.push(String::new());
            self.has_code.push(false);
            self.first_tok.push(None);
        }
    }

    fn push_tok(&mut self, tok: Tok, line: usize) {
        self.ensure(line);
        self.has_code[line] = true;
        if self.first_tok[line].is_none() {
            self.first_tok[line] = Some(tok.clone());
        }
        self.toks.push((tok, line));
    }

    fn push_comment(&mut self, line: usize, text: &str) {
        self.ensure(line);
        if !self.comments[line].is_empty() {
            self.comments[line].push(' ');
        }
        self.comments[line].push_str(text);
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Lexes `src`.
pub fn lex(src: &str) -> Lexed {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Lexed::default();
    let mut line = 1usize;
    out.ensure(line);
    let mut i = 0usize;
    let n = chars.len();
    while i < n {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            out.ensure(line);
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            let start = i;
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            out.push_comment(line, &text);
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            let mut depth = 0usize;
            let mut buf = String::new();
            while i < n {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    buf.push_str("/*");
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    buf.push_str("*/");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else if chars[i] == '\n' {
                    out.push_comment(line, &buf);
                    buf.clear();
                    line += 1;
                    out.ensure(line);
                    i += 1;
                } else {
                    buf.push(chars[i]);
                    i += 1;
                }
            }
            out.push_comment(line, &buf);
        } else if c == '"' {
            let start_line = line;
            i = skip_string(&chars, i, &mut line);
            out.ensure(line);
            out.push_tok(Tok::Lit, start_line);
        } else if c == '\'' {
            // Character literal or lifetime.
            if chars.get(i + 1) == Some(&'\\') {
                let mut j = i + 2;
                while j < n && chars[j] != '\'' && chars[j] != '\n' {
                    j += 1;
                }
                i = (j + 1).min(n);
                out.push_tok(Tok::Lit, line);
            } else if chars.get(i + 2) == Some(&'\'') {
                i += 3;
                out.push_tok(Tok::Lit, line);
            } else {
                // A lifetime or label: skip the tick and the name.
                i += 1;
                while i < n && is_ident_continue(chars[i]) {
                    i += 1;
                }
            }
        } else if is_ident_start(c) {
            let start = i;
            while i < n && is_ident_continue(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let next = chars.get(i).copied();
            if matches!(word.as_str(), "r" | "br" | "cr") && matches!(next, Some('#' | '"')) {
                // Raw string: count the hashes, then the opening quote must follow.
                let mut j = i;
                let mut hashes = 0usize;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if chars.get(j) == Some(&'"') {
                    let start_line = line;
                    j += 1;
                    'raw: while j < n {
                        if chars[j] == '\n' {
                            line += 1;
                        }
                        if chars[j] == '"' {
                            let mut k = 0usize;
                            while k < hashes && chars.get(j + 1 + k) == Some(&'#') {
                                k += 1;
                            }
                            if k == hashes {
                                j += 1 + hashes;
                                break 'raw;
                            }
                        }
                        j += 1;
                    }
                    i = j;
                    out.ensure(line);
                    out.push_tok(Tok::Lit, start_line);
                    continue;
                }
                // A raw identifier such as `r#type`: fall through as an identifier.
                out.push_tok(Tok::Ident(word), line);
            } else if matches!(word.as_str(), "b" | "c") && next == Some('"') {
                let start_line = line;
                i = skip_string(&chars, i, &mut line);
                out.ensure(line);
                out.push_tok(Tok::Lit, start_line);
            } else if word == "b" && next == Some('\'') {
                let mut j = i + 1;
                if chars.get(j) == Some(&'\\') {
                    j += 1;
                }
                j += 1;
                while j < n && chars[j] != '\'' && chars[j] != '\n' {
                    j += 1;
                }
                i = (j + 1).min(n);
                out.push_tok(Tok::Lit, line);
            } else {
                out.push_tok(Tok::Ident(word), line);
            }
        } else if c.is_ascii_digit() {
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push_tok(Tok::Lit, line);
        } else {
            out.push_tok(Tok::Punct(c), line);
            i += 1;
        }
    }
    out
}

/// `i` points at the opening quote; returns the index after the closing quote.
fn skip_string(chars: &[char], mut i: usize, line: &mut usize) -> usize {
    i += 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                if chars.get(i + 1) == Some(&'\n') {
                    *line += 1;
                }
                i += 2;
            }
            '"' => return i + 1,
            '\n' => {
                *line += 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idents(src: &str) -> Vec<String> {
        lex(src)
            .toks
            .into_iter()
            .filter_map(|(t, _)| match t {
                Tok::Ident(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn comments_and_strings_hide_words() {
        let src = "// unsafe here\n/* unsafe\n unsafe */ let a = \"unsafe\"; let b = r#\"unsafe \" unsafe\"#; let c = 'x'; /// unsafe doc\nfn f<'a>() {}\n";
        let ids = idents(src);
        assert!(!ids.iter().any(|s| s == "unsafe"), "{ids:?}");
        assert!(ids.contains(&"fn".to_owned()));
    }

    #[test]
    fn code_is_seen_with_its_line() {
        let l = lex("fn a() {}\n\nunsafe { }\n");
        let (_, line) = l
            .toks
            .iter()
            .find(|(t, _)| *t == Tok::Ident("unsafe".into()))
            .unwrap();
        assert_eq!(*line, 3);
    }

    #[test]
    fn comment_text_is_recorded_per_line() {
        let l = lex("// SAFETY: ok\nunsafe { }\n");
        assert!(l.comments[1].contains("SAFETY:"));
        assert!(!l.has_code[1]);
        assert!(l.has_code[2]);
    }

    #[test]
    fn nested_block_comments() {
        let ids = idents("/* a /* unsafe */ unsafe */ fn x() {}");
        assert_eq!(ids, vec!["fn", "x"]);
    }

    #[test]
    fn byte_and_escaped_literals() {
        let ids = idents("let a = b\"unsafe\"; let b = b'u'; let c = '\\''; let d = unsafe_code;");
        assert!(!ids.iter().any(|s| s == "unsafe"), "{ids:?}");
        assert!(ids.iter().any(|s| s == "unsafe_code"));
    }
}
