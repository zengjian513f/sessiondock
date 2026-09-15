//! The candidate filter of one query over the folded copies of the cached
//! bodies (`search::fold`): a conjunction of clauses, each clause a set of
//! folded literals of which at least one occurs in every body that can
//! match. A body failing a clause is skipped without opening its text; a
//! body passing every clause is matched by the real matcher as before, so
//! the filter can only save work, never change a result.
//!
//! Literal and whole-word queries need one clause: the folded needle. Regex
//! queries take their clauses from the matcher's own parse tree
//! (`fancy_regex::Expr`): a literal node is required wherever it sits in a
//! concatenation, group, positive look-around or repetition with a positive
//! minimum; an alternation requires one literal per branch (one clause
//! with all of them); classes, `.`, back-references, negative look-arounds,
//! conditionals and `*`/`?` repetitions require nothing. A pattern with no
//! required literal has no clause and runs the engine on every body.

use fancy_regex::{Expr, LookAround};
use memchr::memmem::Finder;

use super::fold;

/// At most this many literals in one clause (a wider alternation requires
/// nothing) and this many clauses per query (the strongest are kept).
const CLAUSE_LITERALS: usize = 32;
const CLAUSES: usize = 8;

pub struct Prefilter {
    clauses: Vec<Vec<Finder<'static>>>,
}

type Clause = Vec<Vec<u8>>;

impl Prefilter {
    /// No constraint: every body is a candidate.
    pub fn none() -> Self {
        Self {
            clauses: Vec::new(),
        }
    }

    /// The folded needle of a literal (or whole-word literal) query.
    pub fn literal(needle: &str) -> Self {
        Self::from_clauses(vec![vec![fold::fold(needle)]])
    }

    /// The required literals of a regex query, from its parse tree; the
    /// parse flags are the matcher's (`case_insensitive` only sets the
    /// literals' case flag, which folding makes irrelevant).
    pub fn regex(pattern: &str, case_insensitive: bool) -> Self {
        use fancy_regex::internal::{FLAG_CASEI, FLAG_UNICODE};
        let flags = FLAG_UNICODE | if case_insensitive { FLAG_CASEI } else { 0 };
        match Expr::parse_tree_with_flags(pattern, flags) {
            Ok(tree) => Self::from_clauses(clauses(&tree.expr)),
            Err(_) => Self::none(),
        }
    }

    fn from_clauses(mut clauses: Vec<Clause>) -> Self {
        for clause in &mut clauses {
            clause.sort();
            clause.dedup();
        }
        // A clause with an empty literal is always satisfied.
        clauses.retain(|clause| !clause.is_empty() && clause.iter().all(|lit| !lit.is_empty()));
        clauses.retain(|clause| clause.len() <= CLAUSE_LITERALS);
        clauses.sort_by_key(|clause| std::cmp::Reverse(strength(clause)));
        clauses.dedup();
        clauses.truncate(CLAUSES);
        Self {
            clauses: clauses
                .into_iter()
                .map(|clause| {
                    clause
                        .into_iter()
                        .map(|lit| Finder::new(&lit).into_owned())
                        .collect()
                })
                .collect(),
        }
    }

    pub fn is_none(&self) -> bool {
        self.clauses.is_empty()
    }

    /// The literals of every clause, for tests and diagnostics.
    pub fn literals(&self) -> Vec<Vec<&[u8]>> {
        self.clauses
            .iter()
            .map(|clause| clause.iter().map(Finder::needle).collect())
            .collect()
    }

    /// Whether a body with this folded text can match.
    pub fn admits(&self, folded: &[u8]) -> bool {
        self.clauses
            .iter()
            .all(|clause| clause.iter().any(|finder| finder.find(folded).is_some()))
    }
}

/// The length of a clause's shortest literal: what it guarantees.
fn strength(clause: &Clause) -> usize {
    clause.iter().map(Vec::len).min().unwrap_or(0)
}

/// The conjunction of clauses every match of `expr` satisfies.
fn clauses(expr: &Expr) -> Vec<Clause> {
    match expr {
        Expr::Literal { val, .. } => {
            if val.is_empty() {
                Vec::new()
            } else {
                vec![vec![fold::fold(val)]]
            }
        }
        Expr::Concat(children) => {
            let mut out = Vec::new();
            // Adjacent literals match contiguously: one longer literal.
            let mut run = String::new();
            for child in children {
                if let Expr::Literal { val, .. } = child {
                    run.push_str(val);
                    continue;
                }
                if !run.is_empty() {
                    out.push(vec![fold::fold(&std::mem::take(&mut run))]);
                }
                out.extend(clauses(child));
            }
            if !run.is_empty() {
                out.push(vec![fold::fold(&run)]);
            }
            out
        }
        Expr::Alt(children) => {
            // One clause: the strongest clause of every branch, united.
            let mut union: Clause = Vec::new();
            for child in children {
                let branch = clauses(child);
                let Some(best) = branch.into_iter().max_by_key(strength) else {
                    return Vec::new();
                };
                union.extend(best);
            }
            vec![union]
        }
        Expr::Group(child) => clauses(child),
        Expr::AtomicGroup(child) => clauses(child),
        Expr::Repeat { child, lo, .. } if *lo >= 1 => clauses(child),
        Expr::LookAround(child, LookAround::LookAhead | LookAround::LookBehind) => clauses(child),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lits(filter: &Prefilter) -> Vec<Vec<String>> {
        filter
            .literals()
            .iter()
            .map(|clause| {
                clause
                    .iter()
                    .map(|lit| String::from_utf8_lossy(lit).into_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn literal_queries_fold_the_needle() {
        let filter = Prefilter::literal("ddp_Guard");
        assert_eq!(lits(&filter), [["DDP_GUARD"]]);
        assert!(filter.admits(&fold::fold("x DDP_guard y")));
        assert!(filter.admits(&fold::fold("ddp_guar\u{0064}")));
        assert!(!filter.admits(&fold::fold("ddp guard")));
        assert!(!filter.admits(b""));
        assert!(Prefilter::none().is_none() && Prefilter::none().admits(b""));
        assert!(Prefilter::literal("").is_none());
    }

    #[test]
    fn regex_queries_require_their_literals() {
        let cases: &[(&str, &[&[&str]])] = &[
            ("agenthub.*rust", &[&["AGENTHUB"], &["RUST"]]),
            ("guard|ddp_?guard", &[&["GUARD"]]),
            ("ddp_?guard|xyz", &[&["GUARD", "XYZ"]]),
            ("(?i)Foo(bar)+baz?", &[&["FOO"], &["BAR"], &["BA"]]),
            ("foo(bar)*", &[&["FOO"]]),
            (r"\bguard\b", &[&["GUARD"]]),
            (r"(?<!\w)guard(?!\w)", &[&["GUARD"]]),
            (r"(?<=ddp_)guard", &[&["DDP_"], &["GUARD"]]),
            (r"(?!x)guard", &[&["GUARD"]]),
            (r"(a|b)c", &[&["A", "B"], &["C"]]),
            (r"(a|)c", &[&["C"]]),
            (r"(cat)\1", &[&["CAT"]]),
            (r"[a-z]+", &[]),
            (r".*", &[]),
            (r"a?b*c*", &[]),
            (r"(?:foo)?", &[]),
            (r"x{0,3}", &[]),
            (r"猫\.txt", &[&["猫.TXT"]]),
            (r"\x41b", &[&["AB"]]),
            (r"K", &[&["K"]]),
        ];
        for (pattern, expected) in cases {
            let filter = Prefilter::regex(pattern, true);
            let mut got = lits(&filter);
            let mut want: Vec<Vec<String>> = expected
                .iter()
                .map(|clause| clause.iter().map(|s| s.to_string()).collect())
                .collect();
            got.sort();
            want.sort();
            assert_eq!(got, want, "{pattern}");
            assert_eq!(filter.is_none(), expected.is_empty(), "{pattern}");
        }
        assert!(
            Prefilter::regex("(", true).is_none(),
            "unparsable: no constraint"
        );
    }

    #[test]
    fn every_match_of_a_regex_passes_its_filter() {
        let bodies = [
            "The ddp_guard hook\nsecond line",
            "AGENTHUB was ported to Rust",
            "guard\tddp guard\nDDPGUARD",
            "catcat and Cat",
            "\u{212A}elvin \u{17F}ession",
            "",
            "aaa#a#a",
            "nothing here",
        ];
        for pattern in [
            "agenthub.*rust",
            "guard|ddp_?guard",
            "(?i)ddp_guard",
            r"(cat)\1",
            r"(?<!\w)guard(?!\w)",
            r"(?<=ddp_)guard",
            "kelvin",
            "session",
            "a#a",
            "[a-z]+",
            "x?",
        ] {
            for case_insensitive in [true, false] {
                let engine = fancy_regex::RegexBuilder::new(pattern)
                    .case_insensitive(case_insensitive)
                    .build()
                    .unwrap();
                let filter = Prefilter::regex(pattern, case_insensitive);
                for body in bodies {
                    if engine.is_match(body).unwrap() {
                        assert!(filter.admits(&fold::fold(body)), "{pattern:?} on {body:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn wide_alternations_and_many_clauses_stay_bounded() {
        let wide = (0..40)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join("|");
        assert!(Prefilter::regex(&wide, true).is_none());
        let many = (0..20)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join(".");
        let filter = Prefilter::regex(&many, true);
        assert_eq!(filter.literals().len(), CLAUSES);
        assert!(filter.admits(&fold::fold(&many.replace('.', "-"))));
    }
}
