//! Vector operations: little-endian f32 blob serialization, cosine similarity,
//! and language detection heuristic matching `core/semantic.py`.

use std::error::Error;
use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub struct VectorError(pub String);

impl fmt::Display for VectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for VectorError {}

/// Encode a slice of float32s to bytes matching Python's
/// `struct.pack(f"{len(vec)}f", *vec)` on little-endian platforms.
pub fn encode_f32_vector(vec: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vec.len() * 4);
    for &val in vec {
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

/// Decode a float32 vector from bytes matching Python's
/// `struct.unpack(f"{len(blob) // 4}f", blob)`.
pub fn decode_f32_vector(blob: &[u8]) -> Result<Vec<f32>, VectorError> {
    if !blob.len().is_multiple_of(4) {
        return Err(VectorError(format!(
            "Invalid vector blob length: {} (not a multiple of 4)",
            blob.len()
        )));
    }
    let count = blob.len() / 4;
    let mut vec = Vec::with_capacity(count);
    for chunk in blob.chunks_exact(4) {
        let bytes: [u8; 4] = chunk
            .try_into()
            .map_err(|_| VectorError("Failed to convert slice to [u8; 4]".to_string()))?;
        vec.push(f32::from_le_bytes(bytes));
    }
    Ok(vec)
}

/// Compute cosine similarity between two vector slices (`semantic.py:39-46`).
///
/// Returns 0.0 if dimensions mismatch, either slice is empty, or either norm is 0.0.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a_sq = 0.0f32;
    let mut norm_b_sq = 0.0f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a_sq += x * x;
        norm_b_sq += y * y;
    }
    let norm_a = norm_a_sq.sqrt();
    let norm_b = norm_b_sq.sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Language detection heuristic (`semantic.py:70-75`):
/// If >30% CJK characters (U+4E00..=U+9FFF), return `"zh"`, else `"en"`.
pub fn detect_language(text: &str) -> &'static str {
    if text.is_empty() {
        return "en";
    }
    let mut cjk_count = 0usize;
    let mut char_count = 0usize;
    for c in text.chars() {
        char_count += 1;
        if ('\u{4e00}'..='\u{9fff}').contains(&c) {
            cjk_count += 1;
        }
    }
    if char_count == 0 {
        return "en";
    }
    if (cjk_count as f64) / (char_count as f64) > 0.3 {
        "zh"
    } else {
        "en"
    }
}

/// Round a similarity score to 4 decimal places matching Python `round(score, 4)`.
pub fn round_score(score: f32) -> f64 {
    ((score as f64) * 10000.0).round() / 10000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_round_trip() {
        let original = vec![1.0f32, -2.5, 0.0, std::f32::consts::PI, 100.25];
        let encoded = encode_f32_vector(&original);
        assert_eq!(encoded.len(), original.len() * 4);
        let decoded = decode_f32_vector(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_empty_vector() {
        let empty: Vec<f32> = Vec::new();
        let encoded = encode_f32_vector(&empty);
        assert!(encoded.is_empty());
        let decoded = decode_f32_vector(&encoded).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_invalid_blob_length() {
        let bad_blob = vec![0u8, 1, 2];
        let err = decode_f32_vector(&bad_blob).unwrap_err();
        assert!(err.0.contains("not a multiple of 4"));
    }

    #[test]
    fn test_python_struct_pack_compatibility() {
        // struct.pack("<3f", 1.0, 2.0, 3.0) on little endian:
        // 1.0f32 -> 0x00, 0x00, 0x80, 0x3f
        // 2.0f32 -> 0x00, 0x00, 0x00, 0x40
        // 3.0f32 -> 0x00, 0x00, 0x40, 0x40
        let python_bytes: Vec<u8> = vec![
            0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x40, 0x40,
        ];
        let decoded = decode_f32_vector(&python_bytes).unwrap();
        assert_eq!(decoded, vec![1.0f32, 2.0, 3.0]);

        let re_encoded = encode_f32_vector(&decoded);
        assert_eq!(re_encoded, python_bytes);
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let c = vec![0.0, 1.0, 0.0];
        let d = vec![-1.0, 0.0, 0.0];
        let zero = vec![0.0, 0.0, 0.0];

        assert_eq!(cosine_similarity(&a, &b), 1.0);
        assert_eq!(cosine_similarity(&a, &c), 0.0);
        assert_eq!(cosine_similarity(&a, &d), -1.0);
        assert_eq!(cosine_similarity(&a, &zero), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&a, &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language(""), "en");
        assert_eq!(detect_language("Hello world, this is a test."), "en");
        assert_eq!(detect_language("这是中文测试"), "zh");
        // 3 CJK chars out of 10 chars = 30% -> "en" (must be strictly > 30% per Python > 0.3)
        assert_eq!(detect_language("1234567这是中"), "en");
        // 4 CJK chars out of 10 chars = 40% -> "zh"
        assert_eq!(detect_language("123456这是中文"), "zh");
    }

    #[test]
    fn test_round_score() {
        assert_eq!(round_score(0.85231), 0.8523);
        assert_eq!(round_score(0.85236), 0.8524);
        assert_eq!(round_score(1.0), 1.0);
        assert_eq!(round_score(0.0), 0.0);
    }
}
