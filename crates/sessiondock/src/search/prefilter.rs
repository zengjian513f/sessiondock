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

    pub fn terms(terms: &[String], any: bool) -> Self {
        let literals: Vec<_> = terms.iter().map(|term| fold::fold(term)).collect();
        Self::from_clauses(if any {
            vec![literals]
        } else {
            literals.into_iter().map(|literal| vec![literal]).collect()
        })
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
