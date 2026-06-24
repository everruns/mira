//! Tiny, dependency-free glob matcher for case selection.
//!
//! Selection (`--targets`, `--samples`, `--evals`, presets) matches labels with
//! this rather than a heavy regex engine — the core crate carries no heavy deps
//! (see AGENTS.md). The grammar is the familiar shell glob, anchored (the whole
//! text must match, like `cargo test` would *not* — selection is exact-by-glob):
//!
//! - `*`        any run of characters, including empty
//! - `?`        exactly one character
//! - `[abc]`    one character from the set; `[a-z]` a range; `[!ab]`/`[^ab]` negated
//! - `{a,b,c}`  alternation — matches if any branch matches (nests; a `,` inside
//!   `[...]` is literal)
//!
//! A pattern with no metacharacters is a plain literal, so `targets = ["sim"]`
//! still means exactly `sim`.

/// True if `pattern` matches the whole of `text`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    expand_braces(pattern)
        .iter()
        .any(|p| match_tokens(&tokenize(p), &text.chars().collect::<Vec<_>>()))
}

/// Expand `{a,b}` alternations into the concrete patterns they stand for. A
/// missing/unbalanced `}` leaves the `{` literal, so a stray brace can't drop a
/// pattern. Top-level commas split branches; commas nested in inner braces or in
/// `[...]` are carried into the branch untouched.
fn expand_braces(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let Some(open) = chars.iter().position(|&c| c == '{') else {
        return vec![pattern.to_string()];
    };
    // Find the matching close brace, honoring nesting and `[...]` classes.
    let mut depth = 0usize;
    let mut close = None;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '[' => i = class_end(&chars, i), // skip a class wholesale
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let Some(close) = close else {
        return vec![pattern.to_string()];
    };
    let pre: String = chars[..open].iter().collect();
    let post: String = chars[close + 1..].iter().collect();
    let inner: Vec<char> = chars[open + 1..close].to_vec();
    split_top_commas(&inner)
        .into_iter()
        .flat_map(|branch| expand_braces(&format!("{pre}{branch}{post}")))
        .collect()
}

/// Split `inner` on commas that sit at brace/class depth zero.
fn split_top_commas(inner: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < inner.len() {
        match inner[i] {
            '[' => {
                let end = class_end(inner, i);
                cur.extend(&inner[i..=end.min(inner.len() - 1)]);
                i = end;
            }
            '{' => {
                depth += 1;
                cur.push('{');
            }
            '}' => {
                depth = depth.saturating_sub(1);
                cur.push('}');
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            c => cur.push(c),
        }
        i += 1;
    }
    out.push(cur);
    out
}

/// Index of the `]` closing the class that opens at `start` (a `[`). A `]`
/// immediately after `[` or `[!`/`[^` is a literal member, not the close. If the
/// class never closes, returns the last index so callers treat the rest as
/// literal text.
fn class_end(chars: &[char], start: usize) -> usize {
    let mut i = start + 1;
    if matches!(chars.get(i), Some('!') | Some('^')) {
        i += 1;
    }
    if matches!(chars.get(i), Some(']')) {
        i += 1;
    }
    while i < chars.len() {
        if chars[i] == ']' {
            return i;
        }
        i += 1;
    }
    chars.len() - 1
}

/// A compiled pattern element. Braces are already expanded away.
enum Tok {
    Star,
    Any,
    Lit(char),
    Class { negate: bool, items: Vec<ClassItem> },
}

enum ClassItem {
    Ch(char),
    Range(char, char),
}

/// Tokenize a brace-free pattern. A malformed `[` (no close) is treated as a
/// literal `[`, mirroring shell behavior.
fn tokenize(pattern: &str) -> Vec<Tok> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                // Collapse runs of `*` — they match the same as one.
                if !matches!(toks.last(), Some(Tok::Star)) {
                    toks.push(Tok::Star);
                }
            }
            '?' => toks.push(Tok::Any),
            '[' => {
                let end = class_end(&chars, i);
                if chars[end] == ']' && end > i {
                    toks.push(parse_class(&chars[i + 1..end]));
                    i = end + 1;
                    continue;
                }
                toks.push(Tok::Lit('[')); // unterminated: literal
            }
            c => toks.push(Tok::Lit(c)),
        }
        i += 1;
    }
    toks
}

/// Parse the body of a `[...]` class (the chars between the brackets).
fn parse_class(body: &[char]) -> Tok {
    let mut items = Vec::new();
    let mut i = 0;
    let negate = matches!(body.first(), Some('!') | Some('^'));
    if negate {
        i += 1;
    }
    while i < body.len() {
        // `a-z` range when a `-` sits between two chars (a trailing `-` is literal).
        if i + 2 < body.len() && body[i + 1] == '-' {
            items.push(ClassItem::Range(body[i], body[i + 2]));
            i += 3;
        } else {
            items.push(ClassItem::Ch(body[i]));
            i += 1;
        }
    }
    Tok::Class { negate, items }
}

fn tok_matches(tok: &Tok, c: char) -> bool {
    match tok {
        Tok::Any => true,
        Tok::Lit(x) => *x == c,
        Tok::Class { negate, items } => {
            let hit = items.iter().any(|it| match it {
                ClassItem::Ch(x) => *x == c,
                ClassItem::Range(a, b) => *a <= c && c <= *b,
            });
            hit != *negate
        }
        Tok::Star => unreachable!("star handled by the matcher loop"),
    }
}

/// Anchored match of a token stream against `text`, with linear-time `*`
/// backtracking (remember the last star and retry one char later on a mismatch).
fn match_tokens(toks: &[Tok], text: &[char]) -> bool {
    let (mut ti, mut tj) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while tj < text.len() {
        match toks.get(ti) {
            Some(Tok::Star) => {
                star = Some(ti);
                mark = tj;
                ti += 1;
            }
            Some(tok) if tok_matches(tok, text[tj]) => {
                ti += 1;
                tj += 1;
            }
            _ => match star {
                Some(s) => {
                    ti = s + 1;
                    mark += 1;
                    tj = mark;
                }
                None => return false,
            },
        }
    }
    while matches!(toks.get(ti), Some(Tok::Star)) {
        ti += 1;
    }
    ti == toks.len()
}

#[cfg(test)]
mod tests {
    use super::glob_match;

    #[test]
    fn literal_is_exact() {
        assert!(glob_match("sim", "sim"));
        assert!(!glob_match("sim", "sims"));
        assert!(!glob_match("sim", "asim"));
        assert!(!glob_match("greet", "coding"));
    }

    #[test]
    fn star_matches_runs() {
        assert!(glob_match("anthropic/*", "anthropic/opus"));
        assert!(glob_match("anthropic/*", "anthropic/")); // empty run
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*opus*", "anthropic/opus@x"));
        assert!(glob_match("claude-*", "claude-opus-4-8"));
        assert!(!glob_match("anthropic/*", "openai/gpt"));
        assert!(glob_match("a*b*c", "axxbyyc"));
        assert!(!glob_match("a*b*c", "axxbyy"));
    }

    #[test]
    fn question_matches_one() {
        assert!(glob_match("v?", "v1"));
        assert!(!glob_match("v?", "v"));
        assert!(!glob_match("v?", "v12"));
    }

    #[test]
    fn classes() {
        assert!(glob_match("v[012]", "v1"));
        assert!(!glob_match("v[012]", "v3"));
        assert!(glob_match("[a-z]", "m"));
        assert!(!glob_match("[a-z]", "M"));
        assert!(glob_match("v[!0]", "v1"));
        assert!(!glob_match("v[!0]", "v0"));
        assert!(glob_match("v[^0]", "v1"));
    }

    #[test]
    fn braces_alternate() {
        assert!(glob_match("{france,spain}", "france"));
        assert!(glob_match("{france,spain}", "spain"));
        assert!(!glob_match("{france,spain}", "italy"));
        assert!(glob_match("geo/{france,spain}", "geo/spain"));
        assert!(glob_match("{anthropic,openai}/*", "openai/gpt"));
        // nested braces
        assert!(glob_match("{a,b{c,d}}", "bd"));
    }

    #[test]
    fn malformed_is_literal() {
        // Unterminated class / brace fall back to literal so nothing is silently dropped.
        assert!(glob_match("v[1", "v[1"));
        assert!(glob_match("a{b", "a{b"));
    }

    #[test]
    fn case_key_shapes() {
        let key = "greet/hello@anthropic/opus";
        assert!(glob_match("greet/*", key));
        assert!(glob_match("*@anthropic/*", key));
        assert!(glob_match("greet/hello@*", key));
        assert!(!glob_match("coding/*", key));
    }
}
