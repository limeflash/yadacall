use rust_stemmers::{Algorithm, Stemmer};

/// Lowercase and tokenize Russian and English words, normalizing `ё` to `е`.
pub fn normalize_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            current.push(match character.to_lowercase().next() {
                Some('ё') => 'е',
                Some(lower) => lower,
                None => character,
            });
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Stem Russian and English words for full-text indexing and matching.
pub fn stem_text(text: &str) -> String {
    let russian = Stemmer::create(Algorithm::Russian);
    let english = Stemmer::create(Algorithm::English);
    normalize_tokens(text)
        .iter()
        .map(|token| {
            let algorithm = if token
                .chars()
                .any(|character| matches!(character, 'а'..='я'))
            {
                &russian
            } else {
                &english
            };
            algorithm.stem(token).into_owned()
        })
        .collect::<Vec<_>>()
        .join(" ")
}
