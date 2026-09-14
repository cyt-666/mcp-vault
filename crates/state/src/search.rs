//! Bounded deterministic lexical tokenization shared by note and memory retrieval.
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

/// Build bounded deterministic lexical terms for memory FTS and recall queries.
///
/// Latin/digit runs remain whole tokens. Contiguous Han text emits overlapping
/// bigrams so an unspaced paraphrase can share useful local terms. Output uses
/// only alphanumeric token characters and spaces and is therefore safe to quote
/// into a caller-owned FTS expression.
pub fn memory_search_terms<'a>(values: impl IntoIterator<Item = &'a str>, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    const STOP_WORDS: &[&str] = &[
        "a",
        "an",
        "and",
        "are",
        "did",
        "do",
        "does",
        "for",
        "how",
        "in",
        "is",
        "of",
        "on",
        "or",
        "please",
        "the",
        "to",
        "what",
        "when",
        "where",
        "which",
        "who",
        "why",
        "with",
        "什么",
        "为何",
        "为什么",
        "如何",
        "怎么",
        "是否",
        "这个",
        "那个",
        "请问",
    ];

    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    let push_term = |term: String, terms: &mut Vec<String>, seen: &mut HashSet<String>| {
        if !term.is_empty()
            && !STOP_WORDS.contains(&term.as_str())
            && seen.insert(term.clone())
            && terms.len() < limit
        {
            terms.push(term);
        }
    };

    for value in values {
        let normalized = value
            .nfkc()
            .flat_map(char::to_lowercase)
            .collect::<String>();
        let mut word = String::new();
        let mut han = Vec::new();
        let flush_word =
            |word: &mut String, terms: &mut Vec<String>, seen: &mut HashSet<String>| {
                if word.chars().count() >= 2 {
                    push_term(std::mem::take(word), terms, seen);
                } else {
                    word.clear();
                }
            };
        let flush_han =
            |han: &mut Vec<char>, terms: &mut Vec<String>, seen: &mut HashSet<String>| {
                match han.len() {
                    0 => {}
                    1 => push_term(han[0].to_string(), terms, seen),
                    _ => {
                        for pair in han.windows(2) {
                            push_term(pair.iter().collect(), terms, seen);
                            if terms.len() >= limit {
                                break;
                            }
                        }
                    }
                }
                han.clear();
            };

        for character in normalized.chars() {
            if is_han(character) {
                flush_word(&mut word, &mut terms, &mut seen);
                han.push(character);
            } else if character.is_alphanumeric() {
                flush_han(&mut han, &mut terms, &mut seen);
                word.push(character);
            } else {
                flush_word(&mut word, &mut terms, &mut seen);
                flush_han(&mut han, &mut terms, &mut seen);
            }
            if terms.len() >= limit {
                break;
            }
        }
        flush_word(&mut word, &mut terms, &mut seen);
        flush_han(&mut han, &mut terms, &mut seen);
        if terms.len() >= limit {
            break;
        }
    }
    terms.join(" ")
}

fn is_han(value: char) -> bool {
    matches!(
        value as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
    )
}

#[cfg(test)]
mod multilingual_search_tests {
    use super::memory_search_terms;

    #[test]
    fn search_terms_cover_unspaced_han_and_natural_language_noise() {
        let terms = memory_search_terms(["请问以后项目统一使用 Rust v1.94 吗？"], 64);
        assert!(terms.contains("以后"));
        assert!(terms.contains("项目"));
        assert!(terms.contains("统一"));
        assert!(terms.contains("rust"));
        assert!(terms.contains("v1"));
        assert!(terms.contains("94"));
        assert!(!terms.contains("请问"));
    }

    #[test]
    fn search_terms_are_deduplicated_and_bounded() {
        assert_eq!(memory_search_terms(["Rust rust RUST"], 64), "rust");
        assert_eq!(memory_search_terms(["甲乙丙丁"], 2), "甲乙 乙丙");
    }
}
