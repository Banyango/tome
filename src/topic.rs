//! Topic names and the patterns `on:` triggers subscribe with.
//!
//! A topic is one or more dot-separated segments of `[a-z0-9_-]`, like
//! `review.requested`. In a pattern, `*` matches exactly one segment and a
//! trailing `**` matches one or more.

use serde::Serialize;

/// The prefix of tome's own events; nobody else may publish to it.
pub const RESERVED: &str = "tome";

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

/// Check a topic name.
pub fn check_name(topic: &str) -> Result<(), String> {
    if topic.split('.').all(valid_segment) {
        Ok(())
    } else {
        Err(format!("invalid topic `{topic}`: use dot-separated segments of lowercase letters, digits, `_` and `-`"))
    }
}

/// Whether a topic is one of tome's own (`tome.*`).
pub fn is_reserved(topic: &str) -> bool {
    topic.split('.').next() == Some(RESERVED)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Lit(String),
    /// `*`: exactly one segment.
    One,
    /// A trailing `**`: one or more segments.
    Rest,
}

/// A topic pattern.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pattern {
    segments: Vec<Segment>,
}

impl Serialize for Pattern {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl std::fmt::Display for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts: Vec<&str> = self
            .segments
            .iter()
            .map(|s| match s {
                Segment::Lit(l) => l.as_str(),
                Segment::One => "*",
                Segment::Rest => "**",
            })
            .collect();
        f.write_str(&parts.join("."))
    }
}

impl Pattern {
    pub fn parse(pattern: &str) -> Result<Pattern, String> {
        let parts: Vec<&str> = pattern.split('.').collect();
        let mut segments = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            segments.push(match *part {
                "*" => Segment::One,
                "**" if i + 1 == parts.len() => Segment::Rest,
                "**" => return Err(format!("invalid topic pattern `{pattern}`: `**` may only be the last segment")),
                lit if valid_segment(lit) => Segment::Lit(lit.to_string()),
                _ => {
                    return Err(format!(
                        "invalid topic pattern `{pattern}`: use dot-separated segments of lowercase letters, digits, `_` and `-`, `*` or a trailing `**`"
                    ))
                }
            });
        }
        Ok(Pattern { segments })
    }

    /// A topic the pattern matches, with `test` for each wildcard: where a
    /// test event goes.
    pub fn example(&self) -> String {
        let parts: Vec<&str> = self
            .segments
            .iter()
            .map(|s| match s {
                Segment::Lit(l) => l.as_str(),
                Segment::One | Segment::Rest => "test",
            })
            .collect();
        parts.join(".")
    }

    pub fn matches(&self, topic: &str) -> bool {
        let parts: Vec<&str> = topic.split('.').collect();
        fn go(p: &[Segment], t: &[&str]) -> bool {
            match (p.first(), t.first()) {
                (None, None) => true,
                (Some(Segment::Rest), _) => !t.is_empty(),
                (Some(Segment::One), Some(_)) => go(&p[1..], &t[1..]),
                (Some(Segment::Lit(l)), Some(s)) => l == s && go(&p[1..], &t[1..]),
                _ => false,
            }
        }
        go(&self.segments, &parts)
    }

    /// Whether some topic matches both patterns.
    pub fn overlaps(&self, other: &Pattern) -> bool {
        fn go(a: &[Segment], b: &[Segment]) -> bool {
            match (a.first(), b.first()) {
                (None, None) => true,
                (Some(Segment::Rest), _) => !b.is_empty(),
                (_, Some(Segment::Rest)) => !a.is_empty(),
                (Some(x), Some(y)) => {
                    let same = match (x, y) {
                        (Segment::Lit(l), Segment::Lit(r)) => l == r,
                        _ => true,
                    };
                    same && go(&a[1..], &b[1..])
                }
                _ => false,
            }
        }
        go(&self.segments, &other.segments)
    }
}

/// The lifecycle events tome publishes for runs of `workflow`.
pub fn lifecycle_topics(workflow: &str) -> Vec<String> {
    ["started", "succeeded", "failed", "cancelled"]
        .iter()
        .map(|e| format!("{RESERVED}.run.{workflow}.{e}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Pattern {
        Pattern::parse(s).unwrap()
    }

    #[test]
    fn names() {
        assert!(check_name("review.requested").is_ok());
        assert!(check_name("a_b-1").is_ok());
        for bad in ["", "a..b", "Review", "a.b.", "a b", "a.*"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        assert!(is_reserved("tome.run.x.failed"));
        assert!(is_reserved("tome"));
        assert!(!is_reserved("tomes.x"));
    }

    #[test]
    fn patterns_match_segments() {
        assert!(p("review.requested").matches("review.requested"));
        assert!(!p("review.requested").matches("review.requested.x"));
        assert!(p("tome.run.*.failed").matches("tome.run.implement.failed"));
        assert!(!p("tome.run.*.failed").matches("tome.run.a.b.failed"));
        assert!(p("review.**").matches("review.requested"));
        assert!(p("review.**").matches("review.done.ok"));
        assert!(!p("review.**").matches("review"));
        assert!(p("**").matches("anything.at.all"));
        assert_eq!(p("a.*.**").to_string(), "a.*.**");
        for bad in ["a.**.b", "A", "a..b", "a.b*", ""] {
            assert!(Pattern::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn overlap() {
        assert!(p("a.*").overlaps(&p("a.b")));
        assert!(p("a.**").overlaps(&p("*.b.c")));
        assert!(!p("a.*").overlaps(&p("a.b.c")));
        assert!(!p("a.b").overlaps(&p("a.c")));
        assert!(!p("a.**").overlaps(&p("a")));
        assert!(p("tome.run.*.succeeded").overlaps(&p("tome.run.review.succeeded")));
    }
}
