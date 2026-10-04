//! Error types for the ingestion pipeline.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during PDF ingestion.
#[derive(Error, Debug)]
pub enum IngestionError {
    /// File not found at the specified path.
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    /// The file is not a valid PDF document.
    #[error("Invalid PDF format: {0}")]
    InvalidPdf(String),

    /// Text extraction from PDF failed.
    #[error("Text extraction failed: {0}")]
    ExtractionFailed(String),

    /// Metadata extraction from PDF failed.
    #[error("Metadata extraction failed: {0}")]
    MetadataFailed(String),

    /// IO error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// The source manifest is malformed or inconsistent.
    #[error("Invalid source manifest: {0}")]
    Manifest(String),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result type alias for ingestion operations.
pub type IngestionResult<T> = Result<T, IngestionError>;
