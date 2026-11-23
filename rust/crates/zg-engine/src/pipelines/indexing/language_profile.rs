//! Per-repository embedding-model selection.
//!
//! A workspace indexes one text model, so a polyglot repository has to be
//! served by a single model. Choose it from the dominant source language:
//! that language gets the model measured best for it, and the other languages
//! are still indexed and searchable, just below their own best.
//!
//! The table is fitted on the five adjudicated quality lanes
//! (`benchmarks/quality-lanes/SHAPE_AWARE_FUSION_2026-11-22.md`, sweep issue
//! #66). It is a measured preference, not a claim about unseen languages: an
//! unknown or tied mix falls back to the generalist default.

/// Source languages with a measured model preference.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum CodeLanguage {
    Go,
    Python,
    Rust,
    TypeScript,
    Java,
}

impl CodeLanguage {
    /// Model measured best for this language on the quality lanes.
    pub(crate) fn preferred_model(self) -> &'static str {
        match self {
            Self::Go | Self::TypeScript => "local/gte-modernbert-base",
            Self::Python => "local/multilingual-e5-small",
            Self::Rust => "local/jina-embeddings-v2-base-code",
            Self::Java => "local/qwen3-embedding-0.6b",
        }
    }
}

/// Maps a repository-relative path to a measured source language.
///
/// Only languages with a dedicated measurement are mapped. An unmapped
/// *source* file still counts toward dominance through [`is_source_like`], so a
/// repository dominated by one (say C++) falls back to the generalist; non-source
/// files such as Markdown or configuration never contribute.
pub(crate) fn language_for_path(path: &std::path::Path) -> Option<CodeLanguage> {
    let extension = path.extension()?.to_str()?;
    Some(match extension.to_ascii_lowercase().as_str() {
        "go" => CodeLanguage::Go,
        "py" | "pyi" => CodeLanguage::Python,
        "rs" => CodeLanguage::Rust,
        "ts" | "tsx" | "mts" | "cts" => CodeLanguage::TypeScript,
        "java" => CodeLanguage::Java,
        _ => return None,
    })
}

/// Reports a source file in a language this table does not map, so it can
/// still compete for dominance instead of being ignored.
pub(crate) fn is_source_like(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some(
            "c" | "h"
                | "cc"
                | "cpp"
                | "cxx"
                | "hpp"
                | "cs"
                | "rb"
                | "php"
                | "kt"
                | "kts"
                | "swift"
                | "scala"
                | "m"
                | "mm"
                | "ex"
                | "exs"
                | "erl"
                | "hrl"
                | "hs"
                | "lua"
                | "pl"
                | "r"
                | "jl"
                | "dart"
                | "vue"
                | "svelte"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
        )
    )
}

/// Picks the language with the largest share of source bytes.
///
/// Ties return `None`: an even split gives no language the right to speak for
/// the repository, and the generalist default serves every language acceptably.
pub(crate) fn dominant_language(
    weights: impl IntoIterator<Item = (CodeLanguage, u64)>,
) -> Option<CodeLanguage> {
    let mut best: Option<(CodeLanguage, u64)> = None;
    let mut tied = false;
    for (language, bytes) in weights {
        if bytes == 0 {
            continue;
        }
        match best {
            None => best = Some((language, bytes)),
            Some((_, current)) if bytes > current => {
                best = Some((language, bytes));
                tied = false;
            }
            Some((_, current)) if bytes == current => tied = true,
            _ => {}
        }
    }
    match best {
        Some((language, _)) if !tied => Some(language),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{CodeLanguage, dominant_language, language_for_path};

    #[test]
    fn unmapped_source_languages_are_recognised() {
        assert!(super::is_source_like(Path::new("src/engine.cpp")));
        assert!(super::is_source_like(Path::new("web/app.jsx")));
        assert!(!super::is_source_like(Path::new("README.md")));
        assert!(!super::is_source_like(Path::new("Cargo.toml")));
        assert_eq!(language_for_path(Path::new("src/engine.cpp")), None);
    }

    #[test]
    fn maps_source_extensions() {
        assert_eq!(
            language_for_path(Path::new("a/b.go")),
            Some(CodeLanguage::Go)
        );
        assert_eq!(
            language_for_path(Path::new("b/c.PY")),
            Some(CodeLanguage::Python)
        );
        assert_eq!(
            language_for_path(Path::new("c/d.tsx")),
            Some(CodeLanguage::TypeScript)
        );
        assert_eq!(language_for_path(Path::new("Cargo.toml")), None);
        assert_eq!(language_for_path(Path::new("README")), None);
    }

    #[test]
    fn dominant_language_wins_on_bytes() {
        let profile = [
            (CodeLanguage::Python, 900),
            (CodeLanguage::Go, 100),
            (CodeLanguage::Java, 50),
        ];
        assert_eq!(dominant_language(profile), Some(CodeLanguage::Python));
    }

    #[test]
    fn a_tie_or_an_empty_profile_falls_back_to_the_generalist() {
        assert_eq!(
            dominant_language([(CodeLanguage::Rust, 500), (CodeLanguage::Java, 500)]),
            None
        );
        assert_eq!(dominant_language([]), None);
        assert_eq!(
            dominant_language([(CodeLanguage::Rust, 0), (CodeLanguage::Go, 0)]),
            None
        );
    }

    #[test]
    fn languages_have_distinct_or_intentional_models() {
        assert_eq!(
            CodeLanguage::Python.preferred_model(),
            "local/multilingual-e5-small"
        );
        assert_eq!(
            CodeLanguage::Go.preferred_model(),
            CodeLanguage::TypeScript.preferred_model()
        );
    }
}
