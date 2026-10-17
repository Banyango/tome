//! Small text rules: names that appear in files and on the command line, and
//! shell quoting.

/// A workflow, parameter or placement name: a letter or `_`, then letters,
/// digits, `_` or `-`.
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `it's` → `'it'\''s'`
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
