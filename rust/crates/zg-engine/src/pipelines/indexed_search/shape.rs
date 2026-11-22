//! Query-shape classification for fusion weighting.
//!
//! A code identifier and an English sentence must not be fused with the same
//! lexical/vector balance. Lexical recall is trustworthy when the query names
//! a symbol, but for a natural-language question BM25 promotes units that
//! share generic tokens with the question rather than units that answer it.
//! Classification uses only the query string: no embedding call, no storage.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueryShape {
    /// A single token, or tokens that all look like code identifiers.
    Identifier,
    /// A sentence-like query: plain words, stopwords, or a question.
    NaturalLanguage,
    /// A short plain phrase or a sentence carrying one identifier token.
    Mixed,
}

/// Minimum token count at which plain words with no stopword are still
/// treated as natural language rather than a short phrase.
const NATURAL_LANGUAGE_MIN_TOKENS: usize = 4;

/// Vector weight for natural-language queries. High enough that a candidate
/// seen only by the lexical leg cannot displace a deep vector hit, low enough
/// that lexical-only candidates still surface when the vector leg is thin.
const NATURAL_LANGUAGE_VECTOR_WEIGHT: f64 = 2.0;

impl QueryShape {
    /// Classifies a query by lexical shape.
    pub(crate) fn classify(query: &str) -> Self {
        let tokens = query.split_whitespace().collect::<Vec<_>>();
        if tokens.len() <= 1 {
            // A single ASCII token is an identifier. A single token with no
            // ASCII letters at all is a script without word spacing (CJK), so
            // it carries sentence structure the lexical leg cannot trust.
            return match tokens.first() {
                Some(token) if has_non_ascii_letter(token) => Self::NaturalLanguage,
                _ => Self::Identifier,
            };
        }

        let mut identifiers = 0usize;
        let mut natural = 0usize;
        for token in &tokens {
            if is_identifier_shaped(token) {
                identifiers += 1;
            } else {
                natural += 1;
            }
        }

        if identifiers > 0 && natural > 0 {
            return Self::Mixed;
        }
        if identifiers > 0 {
            return Self::Identifier;
        }
        if query.contains('?')
            || tokens.iter().any(|token| is_stopword(token))
            || tokens.len() >= NATURAL_LANGUAGE_MIN_TOKENS
        {
            return Self::NaturalLanguage;
        }
        Self::Mixed
    }

    /// Fusion weights as `(lexical, vector)`.
    ///
    /// Identifier queries keep the historical lexical tie preference; mixed
    /// queries balance both legs; natural-language queries let the vector leg
    /// lead so lexical-only candidates cannot displace deep vector hits.
    pub(crate) fn fusion_weights(self) -> (f64, f64) {
        match self {
            Self::Identifier => (1.1, 1.0),
            Self::Mixed => (1.0, 1.0),
            Self::NaturalLanguage => (1.0, NATURAL_LANGUAGE_VECTOR_WEIGHT),
        }
    }
}

/// Reports structural markers of a code identifier: an internal separator
/// (`_`, `::`, `/`, `.`) or an internal camelCase/PascalCase boundary. A
/// leading capital alone is not enough, since a sentence may start with one.
fn is_identifier_shaped(token: &str) -> bool {
    if token.contains('_') || token.contains("::") {
        return true;
    }
    has_internal_separator(token, '/') || has_internal_separator(token, '.') || has_case_boundary(token)
}

/// Reports a letter outside ASCII, which marks a script without word spacing.
fn has_non_ascii_letter(token: &str) -> bool {
    token.chars().any(|ch| !ch.is_ascii() && ch.is_alphabetic())
}

fn has_internal_separator(token: &str, separator: char) -> bool {
    let runes = token.chars().collect::<Vec<_>>();
    runes.iter().enumerate().any(|(index, rune)| {
        *rune == separator
            && index > 0
            && index + 1 < runes.len()
            && runes[index - 1].is_alphanumeric()
            && runes[index + 1].is_alphanumeric()
    })
}

fn has_case_boundary(token: &str) -> bool {
    let runes = token.chars().collect::<Vec<_>>();
    (1..runes.len()).any(|index| {
        let previous = runes[index - 1];
        let current = runes[index];
        if current.is_uppercase() && previous.is_lowercase() {
            return true;
        }
        index + 1 < runes.len()
            && previous.is_uppercase()
            && current.is_uppercase()
            && runes[index + 1].is_lowercase()
    })
}

/// English function words, used only as a sentence-structure signal.
fn is_stopword(token: &str) -> bool {
    let word = token
        .trim_matches(|ch: char| ".,;:?!()\"'`".contains(ch))
        .to_lowercase();
    matches!(
        word.as_str(),
        "a" | "an" | "the" | "this" | "that" | "these" | "those" | "is" | "are" | "was" | "were"
            | "be" | "been" | "being" | "am" | "do" | "does" | "did" | "of" | "in" | "on" | "at"
            | "to" | "for" | "from" | "by" | "with" | "without" | "into" | "onto" | "about" | "as"
            | "and" | "or" | "but" | "not" | "no" | "where" | "when" | "how" | "why" | "what"
            | "which" | "who" | "whom" | "whose" | "their" | "them" | "they" | "our" | "your"
            | "my" | "his" | "her" | "its" | "it" | "we" | "you" | "i" | "he" | "she" | "own"
            | "can" | "cannot" | "could" | "should" | "would" | "will" | "shall" | "may" | "might"
            | "must" | "all" | "any" | "some" | "if" | "then" | "else" | "than" | "so" | "such"
            | "up" | "out" | "over" | "under" | "there"
    )
}

#[cfg(test)]
mod tests {
    use super::QueryShape;

    #[test]
    fn classifies_identifier_queries() {
        for query in ["validateParameters", "Workflows.recordDiscoveredWorkflows", "list_workflows"] {
            assert_eq!(QueryShape::classify(query), QueryShape::Identifier, "{query}");
        }
    }

    #[test]
    fn classifies_natural_language_queries() {
        for query in [
            "Where are remediation workflow parameters validated before execution?",
            "How does selection work",
            "recording the discovered workflows somewhere",
        ] {
            assert_eq!(
                QueryShape::classify(query),
                QueryShape::NaturalLanguage,
                "{query}"
            );
        }
    }

    #[test]
    fn classifies_short_and_mixed_queries() {
        assert_eq!(QueryShape::classify("empty discovery state"), QueryShape::Mixed);
        assert_eq!(
            QueryShape::classify("where is validateParameters called"),
            QueryShape::Mixed
        );
        assert_eq!(QueryShape::classify(""), QueryShape::Identifier);
        assert_eq!(
            QueryShape::classify("ワークフローはどこで検証されますか"),
            QueryShape::NaturalLanguage
        );
    }

    #[test]
    fn natural_language_weights_favor_the_vector_leg() {
        let (lexical, vector) = QueryShape::NaturalLanguage.fusion_weights();
        assert!(vector > lexical);
        let (lexical, vector) = QueryShape::Identifier.fusion_weights();
        assert!(lexical > vector);
    }
}
