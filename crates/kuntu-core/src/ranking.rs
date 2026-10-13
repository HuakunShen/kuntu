//! Launcher-style ranking for file search candidates.
//!
//! The ranker favors filename intent over generic full-path matches: exact
//! basename, prefix, token, and path-component matches score above substring
//! fallbacks.

use std::path::Path;

use crate::model::{MatchKind, SearchCandidate, SearchQuery};

/// Query normalization shared by every candidate in one ranking pass.
pub struct PreparedQuery {
  text: String,
  tokens: Vec<String>,
  compact: String,
  chars: Vec<char>,
}
impl PreparedQuery {
  pub fn new(query: &SearchQuery) -> Self {
    let text = query.query.trim().to_ascii_lowercase();
    let tokens = tokenize(&text);
    let compact = compact_for_match(&text);
    let chars = compact.chars().collect();
    Self {
      text,
      tokens,
      compact,
      chars,
    }
  }
  pub fn score(&self, path: &Path, root_priority: i32) -> (i32, Vec<MatchKind>) {
    let mut matches = Vec::new();
    let mut score = 0;
    if self.text.is_empty() {
      score += 1;
      matches.push(MatchKind::Empty);
    } else {
      let basename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
      let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
      if basename.eq_ignore_ascii_case(&self.text) || stem.eq_ignore_ascii_case(&self.text) {
        score += 1000;
        matches.push(MatchKind::ExactBasename);
      }
      if prefix_ascii_case(basename, &self.text) || prefix_ascii_case(stem, &self.text) {
        score += 800;
        matches.push(MatchKind::BasenamePrefix);
      }
      if !self.tokens.is_empty()
        && self.tokens.iter().all(|q| {
          basename
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .any(|t| prefix_ascii_case(t, q))
        })
      {
        score += 600;
        matches.push(MatchKind::BasenameToken);
      }
      if !self.tokens.is_empty()
        && self.tokens.iter().all(|q| {
          path
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .any(|c| prefix_ascii_case(c, q))
        })
      {
        score += 300;
        matches.push(MatchKind::PathComponent);
      }
      let compact_path = compact_for_match(&path.to_string_lossy());
      if compact_path.contains(&self.compact) {
        score += 100;
        matches.push(MatchKind::Substring);
      } else {
        let compact_basename = compact_for_match(basename);
        if let Some(fuzzy) = ordered_fuzzy_score(&self.chars, &compact_basename, 90) {
          score += fuzzy;
          matches.push(MatchKind::Fuzzy);
        }
      }
      if path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| self.tokens.iter().any(|q| ext.eq_ignore_ascii_case(q)))
      {
        score += 150;
        matches.push(MatchKind::Extension);
      }
    }
    if root_priority != 0 {
      score += root_priority * 10;
      matches.push(MatchKind::RootPriority);
    }
    (score, matches)
  }
}
fn prefix_ascii_case(value: &str, prefix: &str) -> bool {
  value
    .get(..prefix.len())
    .is_some_and(|s| s.eq_ignore_ascii_case(prefix))
}
pub fn score_candidate(
  query: &SearchQuery,
  candidate: &SearchCandidate,
  root_priority: i32,
) -> (i32, Vec<MatchKind>) {
  PreparedQuery::new(query).score(&candidate.path, root_priority)
}

pub fn tokenize(value: &str) -> Vec<String> {
  value
    .split(|ch: char| !ch.is_alphanumeric())
    .filter(|part| !part.is_empty())
    .map(|part| part.to_ascii_lowercase())
    .collect()
}

/// Normalizes a query or candidate path for compact matching.
///
/// File names often contain spaces, dashes, underscores, dots, or path
/// separators that users omit while typing. Compacting to lowercase
/// alphanumeric characters lets `aws pdf`, `aws-pdf`, and `awspdf` share the
/// same comparison surface without changing the original indexed terms.
fn compact_for_match(value: &str) -> String {
  let mut compact = String::with_capacity(value.len());
  for ch in value.chars().filter(|ch| ch.is_alphanumeric()) {
    compact.push(ch.to_ascii_lowercase());
  }
  compact
}

/// Scores an ordered, character-level fuzzy match against a compact target.
///
/// This is the final filename fallback for queries like `awspdf` matching a
/// basename such as `AWS Certified ... .pdf`. The query and target are already
/// compacted before this function runs, so the algorithm only has to answer:
/// "can most query characters be found in the target in the same order?"
///
/// The scoring deliberately stays low:
/// - exact/prefix/token/path-component matches should always outrank it;
/// - dense matches get a small boost because they are more intentional;
/// - matches near the start of the filename get a small boost;
/// - longer queries may miss one character, which gives typo tolerance without
///   making short noisy inputs match too broadly.
///
/// Returning `None` means "not a fuzzy match"; otherwise the returned score is
/// added to the candidate's total ranking score.
fn ordered_fuzzy_score(query_chars: &[char], target: &str, base_score: i32) -> Option<i32> {
  let target_chars = target.chars().collect::<Vec<_>>();
  if query_chars.len() < 3 || target_chars.is_empty() {
    return None;
  }

  let max_misses = if query_chars.len() >= 5 { 1 } else { 0 };
  let mut misses = 0usize;
  let mut matched = 0usize;
  let mut search_from = 0usize;
  let mut first_match = None;
  let mut last_match = 0usize;

  for query_char in query_chars {
    // Continue searching after the previous hit so the query must match in order.
    // For `awspdf`, this accepts `a` -> `w` -> `s` -> ... -> `p` -> `d` -> `f`
    // in a long filename, but rejects targets where the same letters appear out
    // of sequence.
    let Some(offset) = target_chars[search_from..]
      .iter()
      .position(|target_char| target_char == query_char)
    else {
      misses += 1;
      if misses > max_misses {
        return None;
      }
      continue;
    };

    let index = search_from + offset;
    first_match.get_or_insert(index);
    last_match = index;
    matched += 1;
    search_from = index + 1;
  }

  if matched < 3 || matched + misses != query_chars.len() {
    return None;
  }

  let span = first_match.map(|first| last_match - first + 1)?;
  // Dense matches are more likely to be deliberate abbreviations. A sparse
  // match still works, but it gets no density bonus.
  let density_bonus = if span <= matched + 2 {
    25
  } else if span <= matched * 3 {
    15
  } else {
    0
  };
  let start_bonus = match first_match {
    Some(0) => 20,
    Some(1..=3) => 10,
    _ => 0,
  };
  let miss_penalty = i32::try_from(misses).ok()? * 30;
  let score = base_score + density_bonus + start_bonus - miss_penalty;
  (score > 0).then_some(score)
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use crate::{EntryKind, SearchCandidate, SearchQuery};

  use super::*;

  fn candidate(path: &str) -> SearchCandidate {
    SearchCandidate {
      path: PathBuf::from(path),
      kind: EntryKind::File,
      provider: "test".to_string(),
    }
  }

  #[test]
  fn exact_basename_scores_above_prefix() {
    let query = SearchQuery::new("main");
    let (exact, exact_matches) = score_candidate(&query, &candidate("/Users/alice/Dev/main.rs"), 0);
    let (prefix, prefix_matches) =
      score_candidate(&query, &candidate("/Users/alice/Dev/mainland.rs"), 0);

    assert!(exact > prefix);
    assert!(exact_matches.contains(&MatchKind::ExactBasename));
    assert!(prefix_matches.contains(&MatchKind::BasenamePrefix));
  }

  #[test]
  fn token_match_handles_package_json_query() {
    let query = SearchQuery::new("package json");
    let (score, matches) =
      score_candidate(&query, &candidate("/Users/alice/Dev/app/package.json"), 0);

    assert!(score >= 600);
    assert!(matches.contains(&MatchKind::BasenameToken));
    assert!(matches.contains(&MatchKind::Extension));
  }

  #[test]
  fn path_component_match_scores_below_basename_token() {
    let query = SearchQuery::new("docs");
    let (path_score, path_matches) =
      score_candidate(&query, &candidate("/Users/alice/Dev/docs/readme.md"), 0);
    let (name_score, name_matches) =
      score_candidate(&query, &candidate("/Users/alice/Dev/app/docs.md"), 0);

    assert!(name_score > path_score);
    assert!(path_matches.contains(&MatchKind::PathComponent));
    assert!(name_matches.contains(&MatchKind::BasenameToken));
  }

  #[test]
  fn substring_fallback_is_low_but_nonzero() {
    let query = SearchQuery::new("srclib");
    let (score, matches) =
      score_candidate(&query, &candidate("/Users/alice/Dev/app/src/lib.rs"), 0);

    assert_eq!(score, 100);
    assert!(matches.contains(&MatchKind::Substring));
  }

  #[test]
  fn ordered_fuzzy_match_handles_compacted_query_across_basename_tokens() {
    let query = SearchQuery::new("awspdf");
    let (score, matches) = score_candidate(
      &query,
      &candidate("/Users/alice/Downloads/AWS Certified Solutions Architect Associate SAA-C03.pdf"),
      0,
    );

    assert!(score > 0);
    assert!(matches.contains(&MatchKind::Fuzzy));
  }

  #[test]
  fn root_priority_boosts_score() {
    let query = SearchQuery::new("readme");
    let (plain, _) = score_candidate(&query, &candidate("/Users/alice/Dev/readme.md"), 0);
    let (boosted, matches) = score_candidate(&query, &candidate("/Users/alice/Dev/readme.md"), 10);

    assert_eq!(boosted - plain, 100);
    assert!(matches.contains(&MatchKind::RootPriority));
  }
}
