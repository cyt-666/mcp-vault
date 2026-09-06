//! Pure retrieval admission policy shared by runtime and calibration.
use mcp_vault_state::memory_search_terms;
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

fn normalize_content(value: &str) -> String {
    value
        .nfkc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Query evidence is independent of ranking contributions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LexicalEvidence {
    /// Whether query evidence permits recall.
    pub admitted: bool,
    /// Fraction of normalized query terms matched.
    pub coverage: f64,
    /// Number of matched terms.
    pub matched_terms: usize,
    /// Number of eligible query terms.
    pub query_terms: usize,
}

/// Versioned, deliberately small lexical relevance policy. Keyword-style
/// queries need broad concept coverage; natural-language questions may omit
/// the answer value. Their narrow low-coverage exception accepts only strong
/// metadata evidence: multiple terms including an identifier/ASCII label, an
/// exact distinctive label, or the same term corroborated by two labels.
pub const LEXICAL_RELEVANCE_PROFILE: &str = "current-lexical-relevance-v3";
const KEYWORD_MIN_COVERAGE: f64 = 0.75;
const METADATA_KEYWORD_MIN_COVERAGE: f64 = 0.65;
const QUESTION_MIN_COVERAGE: f64 = 0.30;

/// Shared recall lexical admission for memory, notes and benchmark candidates.
pub fn lexical_relevance(
    query: &str,
    content: &str,
    tags: &[String],
    entities: &[String],
) -> LexicalEvidence {
    let normalized_query = normalize_content(query);
    let normalized_content = normalize_content(content);
    if normalized_content.contains(&normalized_query) {
        return LexicalEvidence {
            admitted: true,
            coverage: 1.0,
            matched_terms: 1,
            query_terms: 1,
        };
    }
    const STOP_WORDS: &[&str] = &[
        "about", "are", "be", "been", "can", "could", "did", "do", "does", "for", "from", "had",
        "has", "have", "how", "if", "is", "may", "must", "our", "please", "shall", "should",
        "tell", "that", "the", "this", "what", "when", "where", "which", "who", "why", "will",
        "with", "would", "关于", "为何", "何时", "记得", "哪个", "哪里", "那个", "请问", "如何",
        "是否", "什么", "这个",
    ];
    let mut terms = memory_search_terms([query], 64)
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|term| term.chars().count() >= 2)
        .filter(|term| !STOP_WORDS.contains(&term.as_str()))
        .collect::<Vec<_>>();
    terms.extend(single_letter_identifiers(query));
    terms.sort();
    terms.dedup();
    if terms.is_empty() {
        return LexicalEvidence::default();
    }

    let searchable = memory_search_terms(
        std::iter::once(content)
            .chain(tags.iter().map(String::as_str))
            .chain(entities.iter().map(String::as_str)),
        4_096,
    );
    let mut searchable = lexical_variant_set(searchable.split_whitespace());
    searchable.extend(single_letter_identifiers(content));
    for value in tags.iter().chain(entities) {
        searchable.extend(single_letter_identifiers(value));
    }
    let metadata = memory_search_terms(
        tags.iter()
            .map(String::as_str)
            .chain(entities.iter().map(String::as_str)),
        2_048,
    );
    let mut metadata = lexical_variant_set(metadata.split_whitespace());
    for value in tags.iter().chain(entities) {
        metadata.extend(single_letter_identifiers(value));
    }
    let identifiers = single_letter_identifiers(query)
        .into_iter()
        .chain(
            terms
                .iter()
                .filter(|term| term.chars().any(|value| value.is_ascii_digit()))
                .cloned(),
        )
        .collect::<HashSet<_>>();
    let matched_terms = terms
        .iter()
        .filter(|term| {
            lexical_variants(term)
                .iter()
                .any(|term| searchable.contains(term))
        })
        .count();
    let coverage = matched_terms as f64 / terms.len() as f64;
    let question = question_like(query);
    let metadata_matches = terms
        .iter()
        .filter(|term| {
            lexical_variants(term)
                .iter()
                .any(|term| metadata.contains(term))
        })
        .count();
    let identifier_match = identifiers.iter().any(|term| searchable.contains(term));
    let metadata_question_admission = question
        && matched_terms >= 1
        && question_metadata_admission(&terms, tags, entities, metadata_matches, identifier_match);
    let admitted = if question {
        (matched_terms >= 2 && coverage >= QUESTION_MIN_COVERAGE) || metadata_question_admission
    } else {
        matched_terms >= 2 && {
            !query_conflicts_with_negated_content(query, content)
                && (coverage >= KEYWORD_MIN_COVERAGE
                    || (coverage >= METADATA_KEYWORD_MIN_COVERAGE
                        && metadata_matches == matched_terms))
        }
    };
    LexicalEvidence {
        admitted,
        coverage,
        matched_terms,
        query_terms: terms.len(),
    }
}

fn question_metadata_admission(
    terms: &[String],
    tags: &[String],
    entities: &[String],
    metadata_matches: usize,
    identifier_match: bool,
) -> bool {
    let metadata_values = tags.iter().chain(entities).collect::<Vec<_>>();
    let matched_metadata_terms = terms
        .iter()
        .filter(|term| {
            metadata_values.iter().any(|value| {
                let value_terms = lexical_variant_set(
                    memory_search_terms([value.as_str()], 64).split_whitespace(),
                );
                lexical_variants(term)
                    .iter()
                    .any(|variant| value_terms.contains(variant))
            })
        })
        .collect::<Vec<_>>();

    if metadata_matches >= 2
        && (identifier_match
            || matched_metadata_terms
                .iter()
                .any(|term| term.is_ascii() && term.chars().count() >= 3))
    {
        return true;
    }

    const WEAK_EXACT_LABELS: &[&str] = &[
        "内容", "信息", "数据", "服务", "状态", "系统", "记忆", "计划", "进度", "配置", "项目",
    ];
    matched_metadata_terms.into_iter().any(|term| {
        let exact_distinctive_label = !WEAK_EXACT_LABELS.contains(&term.as_str())
            && metadata_values
                .iter()
                .any(|value| normalize_content(value) == *term);
        let variants = lexical_variants(term);
        let corroborating_labels = metadata_values
            .iter()
            .filter(|value| {
                let value_terms = lexical_variant_set(
                    memory_search_terms([value.as_str()], 64).split_whitespace(),
                );
                variants.iter().any(|variant| value_terms.contains(variant))
            })
            .take(2)
            .count();
        exact_distinctive_label || corroborating_labels >= 2
    })
}

fn lexical_variant_set<'a>(terms: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
    terms
        .into_iter()
        .flat_map(lexical_variants)
        .collect::<HashSet<_>>()
}

fn lexical_variants(term: &str) -> Vec<String> {
    let mut variants = vec![term.to_owned()];
    if term.is_ascii() && term.chars().all(char::is_alphanumeric) {
        for suffix in ["ingly", "edly", "ing", "ly", "ies", "ed", "es", "s"] {
            if let Some(stem) = term.strip_suffix(suffix)
                && stem.len() >= 3
            {
                variants.push(stem.to_owned());
                if suffix == "ed" || suffix == "es" {
                    variants.push(format!("{stem}e"));
                }
            }
        }
    }
    variants
}

fn single_letter_identifiers(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| part.len() == 1 && part.as_bytes()[0].is_ascii_uppercase())
        .map(str::to_lowercase)
        .collect()
}

fn question_like(value: &str) -> bool {
    let normalized = value.to_lowercase();
    normalized.contains('?')
        || [
            "what ", "which ", "where ", "when ", "who ", "why ", "how ", "can ", "could ",
            "does ", "do ", "is ", "are ", "should ",
        ]
        .iter()
        .any(|marker| normalized.starts_with(marker))
        || [
            "什么", "哪个", "哪次", "哪里", "何时", "为何", "如何", "是否", "吗", "几点",
        ]
        .iter()
        .any(|marker| value.contains(marker))
}

/// An assertive keyword query must not turn an explicitly negated claim into
/// a positive hit merely because stemming made the words look identical.
/// Natural-language questions are excluded: a negative sentence may be the
/// correct answer to a yes/no question.
fn query_conflicts_with_negated_content(query: &str, content: &str) -> bool {
    let query_terms = lexical_variant_set(memory_search_terms([query], 64).split_whitespace());
    let normalized_content = normalize_content(content);
    query_terms.iter().any(|term| {
        ["not ", "not a ", "not an ", "never ", "no "]
            .iter()
            .any(|prefix| normalized_content.contains(&format!("{prefix}{term}")))
            || ["不", "未", "非", "无"]
                .iter()
                .any(|prefix| normalized_content.contains(&format!("{prefix}{term}")))
    })
}

/// Semantic admission and object-rank contribution shared with calibration.
pub fn calibrated_semantic_rank_score(
    similarity: f32,
    rank: usize,
    min_cosine: f64,
) -> Option<f64> {
    let similarity = f64::from(similarity);
    if !similarity.is_finite() || similarity < min_cosine || !(0.0..1.0).contains(&min_cosine) {
        return None;
    }
    let normalized = ((similarity - min_cosine) / (1.0 - min_cosine)).clamp(0.0, 1.0);
    Some(0.20 + 0.45 * normalized + 0.05 / (rank as f64 + 1.0))
}

/// Production memory lexical coverage and candidate-rank contributions.
pub fn memory_lexical_contributions(coverage: f64, rank: usize) -> (f64, f64) {
    (0.72 * coverage, 0.08 / (rank as f64 + 1.0))
}
/// Production ordinary-note lexical rank contribution.
pub fn note_lexical_contribution(rank: usize) -> f64 {
    1.0 / (60.0 + rank as f64 + 1.0)
}
