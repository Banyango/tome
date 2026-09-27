//! Path globs for file triggers: `*` and `?` within a path segment, `**` for
//! any number of segments, `[abc]` / `[!a-z]` classes and `{a,b}`
//! alternatives. Matching is on `/`-separated paths.

#[derive(Debug, Clone)]
pub struct Glob {
    pub source: String,
    /// One segment list per `{a,b}` expansion.
    alternatives: Vec<Vec<Segment>>,
}

#[derive(Debug, Clone, PartialEq)]
enum Segment {
    /// `**`: zero or more whole segments.
    Any,
    Pattern(Vec<Token>),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Char(char),
    /// `?`
    One,
    /// `*`
    Star,
    Class { negated: bool, ranges: Vec<(char, char)> },
}

impl Glob {
    pub fn new(pattern: &str) -> Result<Glob, String> {
        if pattern.trim().is_empty() {
            return Err("empty glob".into());
        }
        let mut alternatives = Vec::new();
        for expanded in expand_braces(pattern)? {
            let mut segments = Vec::new();
            for (i, seg) in expanded.split('/').enumerate() {
                if seg.is_empty() {
                    // A leading `/` (absolute) or a doubled `//`.
                    if i == 0 {
                        segments.push(Segment::Pattern(Vec::new()));
                    }
                    continue;
                }
                if seg == "**" {
                    if segments.last() != Some(&Segment::Any) {
                        segments.push(Segment::Any);
                    }
                } else if seg.contains("**") {
                    return Err(format!("invalid glob `{pattern}`: `**` must be a whole path segment"));
                } else {
                    segments.push(Segment::Pattern(tokens(seg).map_err(|e| format!("invalid glob `{pattern}`: {e}"))?));
                }
            }
            alternatives.push(segments);
        }
        Ok(Glob { source: pattern.to_string(), alternatives })
    }

    /// Whether `path` (`/`-separated, no trailing slash) matches.
    pub fn matches(&self, path: &str) -> bool {
        let parts = split(path);
        self.alternatives.iter().any(|segs| match_segments(segs, &parts))
    }

    /// Whether some path inside directory `dir` could match: used to skip
    /// walking directories that can't contain a match.
    pub fn could_contain(&self, dir: &str) -> bool {
        let parts = split(dir);
        self.alternatives.iter().any(|segs| prefix_possible(segs, &parts))
    }

    /// `/abs/...` or `~/...`.
    pub fn is_absolute(pattern: &str) -> bool {
        pattern.starts_with('/') || pattern.starts_with("~/")
    }
}

/// Path segments; a leading empty one marks an absolute path.
fn split(path: &str) -> Vec<&str> {
    if path.is_empty() {
        return Vec::new();
    }
    path.split('/').enumerate().filter(|(i, p)| *i == 0 || !p.is_empty()).map(|(_, p)| p).collect()
}

fn prefix_possible(segs: &[Segment], dir: &[&str]) -> bool {
    match (segs.first(), dir.first()) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(Segment::Any), Some(_)) => true,
        (Some(Segment::Pattern(t)), Some(d)) => match_tokens(t, &d.chars().collect::<Vec<_>>()) && prefix_possible(&segs[1..], &dir[1..]),
    }
}

fn match_segments(segs: &[Segment], parts: &[&str]) -> bool {
    match segs.first() {
        None => parts.is_empty(),
        Some(Segment::Any) => (0..=parts.len()).any(|skip| match_segments(&segs[1..], &parts[skip..])),
        Some(Segment::Pattern(t)) => match parts.first() {
            Some(p) => match_tokens(t, &p.chars().collect::<Vec<_>>()) && match_segments(&segs[1..], &parts[1..]),
            None => false,
        },
    }
}

fn match_tokens(tokens: &[Token], s: &[char]) -> bool {
    match tokens.first() {
        None => s.is_empty(),
        Some(Token::Star) => (0..=s.len()).any(|skip| match_tokens(&tokens[1..], &s[skip..])),
        Some(t) => match s.first() {
            None => false,
            Some(&c) => {
                let ok = match t {
                    Token::Char(x) => *x == c,
                    Token::One => true,
                    Token::Class { negated, ranges } => ranges.iter().any(|&(a, b)| a <= c && c <= b) != *negated,
                    Token::Star => unreachable!(),
                };
                ok && match_tokens(&tokens[1..], &s[1..])
            }
        },
    }
}

fn tokens(seg: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = seg.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                if out.last() != Some(&Token::Star) {
                    out.push(Token::Star);
                }
            }
            '?' => out.push(Token::One),
            '[' => {
                let mut j = i + 1;
                let negated = matches!(chars.get(j), Some('!') | Some('^'));
                if negated {
                    j += 1;
                }
                let mut ranges = Vec::new();
                let mut first = true;
                loop {
                    let Some(&c) = chars.get(j) else { return Err("unclosed `[`".into()) };
                    if c == ']' && !first {
                        break;
                    }
                    first = false;
                    if chars.get(j + 1) == Some(&'-') && chars.get(j + 2).is_some_and(|&e| e != ']') {
                        let end = chars[j + 2];
                        if end < c {
                            return Err(format!("invalid range `{c}-{end}`"));
                        }
                        ranges.push((c, end));
                        j += 3;
                    } else {
                        ranges.push((c, c));
                        j += 1;
                    }
                }
                out.push(Token::Class { negated, ranges });
                i = j;
            }
            ']' => return Err("unmatched `]`".into()),
            '\\' => {
                i += 1;
                match chars.get(i) {
                    Some(&c) => out.push(Token::Char(c)),
                    None => return Err("trailing `\\`".into()),
                }
            }
            c => out.push(Token::Char(c)),
        }
        i += 1;
    }
    Ok(out)
}

/// `a/{b,c}/*.{md,txt}` into every combination.
fn expand_braces(pattern: &str) -> Result<Vec<String>, String> {
    let Some(open) = pattern.find('{') else {
        if pattern.contains('}') {
            return Err(format!("invalid glob `{pattern}`: unmatched `}}`"));
        }
        return Ok(vec![pattern.to_string()]);
    };
    let mut depth = 0;
    let mut close = None;
    let mut commas = Vec::new();
    for (i, c) in pattern[open..].char_indices().map(|(i, c)| (i + open, c)) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            ',' if depth == 1 => commas.push(i),
            _ => {}
        }
    }
    let Some(close) = close else { return Err(format!("invalid glob `{pattern}`: unclosed `{{`")) };
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    let mut bounds = vec![open];
    bounds.extend(&commas);
    bounds.push(close);
    let mut out = Vec::new();
    for w in bounds.windows(2) {
        let alt = &pattern[w[0] + 1..w[1]];
        out.extend(expand_braces(&format!("{head}{alt}{tail}"))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, path: &str) -> bool {
        Glob::new(p).unwrap().matches(path)
    }

    #[test]
    fn matching() {
        assert!(m("specs/**/*.md", "specs/a.md"));
        assert!(m("specs/**/*.md", "specs/x/y/a.md"));
        assert!(!m("specs/**/*.md", "specs/a.txt"));
        assert!(!m("specs/*.md", "specs/x/a.md"));
        assert!(m("**/*.rs", "src/main.rs"));
        assert!(m("**", "any/thing"));
        assert!(m("a?c.[mt]d", "abc.md"));
        assert!(!m("a[!b]c", "abc"));
        assert!(m("*.{md,txt}", "notes.txt"));
        assert!(m("/abs/*.md", "/abs/x.md"));
        assert!(!m("/abs/*.md", "abs/x.md"));
        assert!(m("docs", "docs"));
    }

    #[test]
    fn prefixes() {
        let g = Glob::new("specs/**/*.md").unwrap();
        assert!(g.could_contain("specs"));
        assert!(g.could_contain("specs/deep/er"));
        assert!(!g.could_contain("src"));
        assert!(Glob::new("*.md").unwrap().could_contain(""));
    }

    #[test]
    fn errors() {
        for bad in ["", "a[bc", "a{b,c", "a**b/x", "x]", "a}"] {
            assert!(Glob::new(bad).is_err(), "{bad}");
        }
    }
}
