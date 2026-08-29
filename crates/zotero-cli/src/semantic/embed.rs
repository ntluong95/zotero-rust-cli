//! Embedding API client matching `core/semantic.py`'s OpenAI-compatible endpoint call.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::error::DomainError;
use crate::paths;

#[derive(Debug, Clone)]
pub struct SemanticConfig {
    pub embed_api: String,
    pub embed_model: String,
    pub embed_key: String,
    pub vector_db: PathBuf,
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self::from_env()
    }
}

impl SemanticConfig {
    pub fn from_env() -> Self {
        let embed_api = std::env::var("ZOTERO_EMBED_API")
            .unwrap_or_else(|_| "http://127.0.0.1:8080/v1/embeddings".to_string());
        let embed_model =
            std::env::var("ZOTERO_EMBED_MODEL").unwrap_or_else(|_| "nomic-embed-text".to_string());
        let embed_key = std::env::var("ZOTERO_EMBED_KEY").unwrap_or_default();
        let vector_db = std::env::var("ZOTERO_VECTOR_DB")
            .map(|p| paths::expand_user_path(&p))
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("Zotero")
                    .join("cli-anything-vectors.sqlite")
            });

        Self {
            embed_api,
            embed_model,
            embed_key,
            vector_db,
        }
    }
}

/// Request embedding vector from OpenAI-compatible embedding API (`semantic.py:23-36`).
pub fn get_embedding(text: &str, config: &SemanticConfig) -> Result<Vec<f32>, DomainError> {
    let body = serde_json::json!({
        "input": text,
        "model": config.embed_model,
    });

    let mut req = ureq::post(&config.embed_api).header("Content-Type", "application/json");

    if !config.embed_key.is_empty() {
        req = req.header("Authorization", &format!("Bearer {}", config.embed_key));
    }

    let response = req
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .send_json(&body)
        .map_err(|e| DomainError::new(format!("{e}")))?;

    let status = response.status().as_u16();
    if status != 200 {
        return Err(DomainError::new(format!("HTTP {status}")));
    }

    let json_resp: Value = response
        .into_body()
        .read_json()
        .map_err(|e| DomainError::new(format!("Failed to parse JSON response: {e}")))?;

    let embedding_values = json_resp
        .get("data")
        .and_then(|d| d.get(0))
        .and_then(|item| item.get("embedding"))
        .and_then(|emb| emb.as_array())
        .ok_or_else(|| {
            DomainError::new("Malformed embedding API response: missing data[0].embedding")
        })?;

    let mut vec = Vec::with_capacity(embedding_values.len());
    for v in embedding_values {
        if let Some(f) = v.as_f64() {
            vec.push(f as f32);
        } else {
            return Err(DomainError::new("Embedding value is not a numeric float"));
        }
    }

    Ok(vec)
}
