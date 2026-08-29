use zotero_cli::semantic::vectors::*;

#[test]
fn test_vector_storage_roundtrip() {
    let original = vec![0.12345f32, -0.98765, 0.0, 42.0, -1337.5];
    let encoded = encode_f32_vector(&original);
    assert_eq!(encoded.len(), 5 * 4);
    let decoded = decode_f32_vector(&encoded).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_empty_vector_storage() {
    let empty: Vec<f32> = vec![];
    let encoded = encode_f32_vector(&empty);
    assert_eq!(encoded.len(), 0);
    let decoded = decode_f32_vector(&encoded).unwrap();
    assert_eq!(decoded.len(), 0);
}

#[test]
fn test_malformed_vector_blob() {
    // 3 bytes is not a multiple of 4
    let bad_bytes = vec![0x00, 0x00, 0x80];
    let err = decode_f32_vector(&bad_bytes).unwrap_err();
    assert!(err.to_string().contains("not a multiple of 4"));

    // 5 bytes
    let bad_bytes5 = vec![0x00, 0x00, 0x80, 0x3f, 0x01];
    let err5 = decode_f32_vector(&bad_bytes5).unwrap_err();
    assert!(err5.to_string().contains("not a multiple of 4"));
}

#[test]
fn test_python_struct_pack_endian_compatibility() {
    // 1.0f32, 2.0f32, -0.5f32 in Python struct.pack("<3f", ...)
    // 1.0f32 = 0x3f800000 -> LE: [0x00, 0x00, 0x80, 0x3f]
    // 2.0f32 = 0x40000000 -> LE: [0x00, 0x00, 0x00, 0x40]
    // -0.5f32 = 0xbf000000 -> LE: [0x00, 0x00, 0x00, 0xbf]
    let python_blob: Vec<u8> = vec![
        0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0xbf,
    ];
    let decoded = decode_f32_vector(&python_blob).unwrap();
    assert_eq!(decoded, vec![1.0f32, 2.0f32, -0.5f32]);

    let re_encoded = encode_f32_vector(&decoded);
    assert_eq!(re_encoded, python_blob);
}

#[test]
fn test_cosine_similarity_edge_cases() {
    // Identical
    let v1 = vec![0.5f32, 0.5, 0.5, 0.5];
    assert!((cosine_similarity(&v1, &v1) - 1.0).abs() < 1e-5);

    // Orthogonal
    let v_x = vec![1.0f32, 0.0];
    let v_y = vec![0.0f32, 1.0];
    assert_eq!(cosine_similarity(&v_x, &v_y), 0.0);

    // Opposite
    let v_neg = vec![-0.5f32, -0.5, -0.5, -0.5];
    assert!((cosine_similarity(&v1, &v_neg) - (-1.0)).abs() < 1e-5);

    // Zero vector
    let zero = vec![0.0f32, 0.0, 0.0];
    let non_zero = vec![1.0f32, 2.0, 3.0];
    assert_eq!(cosine_similarity(&zero, &non_zero), 0.0);
    assert_eq!(cosine_similarity(&non_zero, &zero), 0.0);

    // Dimension mismatch
    let v_dim2 = vec![1.0f32, 2.0];
    let v_dim3 = vec![1.0f32, 2.0, 3.0];
    assert_eq!(cosine_similarity(&v_dim2, &v_dim3), 0.0);

    // Empty slices
    assert_eq!(cosine_similarity(&[], &[]), 0.0);
    assert_eq!(cosine_similarity(&v_dim2, &[]), 0.0);
}

#[test]
fn test_known_cosine_score() {
    // a = [3, 4], b = [4, 3]
    // dot = 12 + 12 = 24
    // norm_a = 5, norm_b = 5
    // cos = 24 / 25 = 0.96
    let a = vec![3.0f32, 4.0];
    let b = vec![4.0f32, 3.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - 0.96).abs() < 1e-5);
    assert_eq!(round_score(sim), 0.96);
}

#[test]
fn test_language_detection_exhaustive() {
    assert_eq!(detect_language(""), "en");
    assert_eq!(
        detect_language("Deep learning and artificial intelligence in medicine"),
        "en"
    );
    assert_eq!(detect_language("深度学习与人工智能在医学中的应用"), "zh");
    // English text with a few Chinese characters (<30%)
    assert_eq!(
        detect_language("This is a study about Beijing (北京) and Shanghai (上海)"),
        "en"
    );
    // Chinese text with English words (>30% CJK)
    assert_eq!(
        detect_language("使用 PyTorch 和 TensorFlow 进行自然语言处理研究"),
        "zh"
    );
}
