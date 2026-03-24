use anyhow::{Context, Result};
use fst::Set;
use std::path::Path;

/// Dictionary loaded from a pre-compiled FST for fast lookups
pub struct Dictionary {
    /// FST set for O(1) word validation and prefix matching
    fst: Set<Vec<u8>>,
    word_count: usize,
}

impl Dictionary {
    /// Load dictionary from a pre-compiled FST file
    pub fn load_from_fst<P: AsRef<Path>>(path: P) -> Result<Self> {
        let fst_bytes = std::fs::read(path.as_ref()).context("Failed to read FST file")?;

        // Count words by iterating once
        let fst = Set::new(fst_bytes).context("Failed to parse FST data")?;

        use fst::Streamer;
        let mut count = 0;
        let mut stream = fst.stream();
        while stream.next().is_some() {
            count += 1;
        }

        Ok(Dictionary {
            fst,
            word_count: count,
        })
    }

    /// Check if a word is valid
    pub fn is_valid(&self, word: &str) -> bool {
        let upper = word.to_ascii_uppercase();
        self.fst.contains(upper.as_bytes())
    }

    /// Check if a prefix could lead to a valid word
    pub fn has_prefix(&self, prefix: &str) -> bool {
        if prefix.is_empty() {
            return true;
        }

        let upper = prefix.to_ascii_uppercase();
        let bytes = upper.as_bytes();

        // Use FST range query to check for prefix
        use fst::{IntoStreamer, Streamer};
        let mut stream = self.fst.range().ge(bytes).into_stream();

        if let Some(word) = stream.next() {
            word.starts_with(bytes)
        } else {
            false
        }
    }

    /// Get the total number of words in the dictionary
    pub fn len(&self) -> usize {
        self.word_count
    }
}

/// Load the default dictionary from the data directory
pub fn load_default_dictionary() -> Result<Dictionary> {
    // Try to find the dictionary relative to the executable
    let exe_path = std::env::current_exe().ok();
    let exe_dir = exe_path.as_ref().and_then(|p| p.parent());

    // Possible locations to check
    let candidates = [
        // Relative to executable (for installed binary)
        exe_dir.map(|p| p.join("data/words.fst")),
        exe_dir.map(|p| p.join("../data/words.fst")),
        // Relative to current working directory (for development)
        Some(std::path::PathBuf::from("data/words.fst")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return Dictionary::load_from_fst(&candidate);
        }
    }

    anyhow::bail!(
        "Could not find words.fst dictionary.\n\
         Run: cargo run --bin build-wordlist <input.txt> data/words.fst"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_dictionary() {
        let dict = load_default_dictionary().unwrap();
        assert!(dict.len() > 100000);
    }

    #[test]
    fn test_word_validation() {
        let dict = load_default_dictionary().unwrap();

        // Valid words
        assert!(dict.is_valid("WORD"));
        assert!(dict.is_valid("word"));
        assert!(dict.is_valid("QUARTZ"));
        assert!(dict.is_valid("AA")); // Valid 2-letter word

        // Invalid words
        assert!(!dict.is_valid("XYZZY"));
        assert!(!dict.is_valid("ASDFGH"));
    }

    #[test]
    fn test_prefix_matching() {
        let dict = load_default_dictionary().unwrap();

        assert!(dict.has_prefix("QU"));
        assert!(dict.has_prefix("QUAR"));
        assert!(dict.has_prefix(""));

        // QX is not a valid prefix
        assert!(!dict.has_prefix("QX"));
    }
}
