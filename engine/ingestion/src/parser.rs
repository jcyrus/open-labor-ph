//! PDF parsing and text extraction module.
//!
//! Provides utilities for extracting text and metadata from PDF documents,
//! specifically designed for Philippine Labor Law documents (DOLE Orders,
//! Labor Code, etc.).

use std::path::Path;

use pdf_extract::extract_text as pdf_extract_text;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use crate::errors::{IngestionError, IngestionResult};

/// Metadata extracted from a PDF document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PdfMetadata {
    /// Document title (if available in PDF metadata).
    pub title: Option<String>,

    /// Document author (if available).
    pub author: Option<String>,

    /// Document subject/description.
    pub subject: Option<String>,

    /// PDF creator application.
    pub creator: Option<String>,

    /// Total number of pages.
    pub page_count: usize,

    /// File size in bytes.
    pub file_size: u64,
}

/// Represents a single page of extracted text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfPage {
    /// Page number (1-indexed).
    pub number: usize,

    /// Extracted text content.
    pub content: String,
}

/// A parsed PDF document with extracted text and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfDocument {
    /// Source file path.
    pub source_path: String,

    /// Extracted metadata.
    pub metadata: PdfMetadata,

    /// Full extracted text content.
    pub full_text: String,

    /// Per-page extracted text.
    pub pages: Vec<PdfPage>,
}

/// Extract all text from a PDF file.
///
/// # Arguments
/// * `path` - Path to the PDF file
///
/// # Returns
/// The full extracted text as a single string.
///
/// # Errors
/// Returns an error if the file doesn't exist, is not a valid PDF,
/// or text extraction fails.
#[instrument(level = "debug", skip_all, fields(path = %path.as_ref().display()))]
pub fn extract_text<P: AsRef<Path>>(path: P) -> IngestionResult<String> {
    let path = path.as_ref();

    // Check file exists
    if !path.exists() {
        return Err(IngestionError::FileNotFound(path.to_path_buf()));
    }

    debug!("Extracting text from PDF");

    // Use pdf-extract for text extraction
    let text = pdf_extract_text(path).map_err(|e| {
        warn!(error = %e, "PDF text extraction failed");
        IngestionError::ExtractionFailed(e.to_string())
    })?;

    debug!(chars = text.len(), "Text extraction complete");
    Ok(text)
}

/// Extract text from a PDF file with page-by-page breakdown.
///
/// # Arguments
/// * `path` - Path to the PDF file
///
/// # Returns
/// A vector of `PdfPage` structs, one for each page.
///
/// # Note
/// The `pdf-extract` crate doesn't provide native page-by-page extraction,
/// so this currently returns the full text as a single page. For proper
/// per-page extraction, we would need to use `lopdf` directly.
#[instrument(level = "debug", skip_all, fields(path = %path.as_ref().display()))]
pub fn extract_pages<P: AsRef<Path>>(path: P) -> IngestionResult<Vec<PdfPage>> {
    let path = path.as_ref();

    // Extract full text first
    let full_text = extract_text(path)?;

    // For now, return as single page since pdf-extract doesn't support
    // page-by-page extraction natively. Future enhancement: use lopdf.
    let pages = vec![PdfPage {
        number: 1,
        content: full_text,
    }];

    Ok(pages)
}

/// Extract metadata from a PDF file.
///
/// # Arguments
/// * `path` - Path to the PDF file
///
/// # Returns
/// A `PdfMetadata` struct with available document information.
#[instrument(level = "debug", skip_all, fields(path = %path.as_ref().display()))]
pub fn extract_metadata<P: AsRef<Path>>(path: P) -> IngestionResult<PdfMetadata> {
    let path = path.as_ref();

    // Check file exists
    if !path.exists() {
        return Err(IngestionError::FileNotFound(path.to_path_buf()));
    }

    // Get file size
    let file_size = std::fs::metadata(path)?.len();

    // Load PDF with lopdf for metadata extraction
    let doc = lopdf::Document::load(path).map_err(|e| {
        warn!(error = %e, "Failed to load PDF for metadata");
        IngestionError::InvalidPdf(e.to_string())
    })?;

    let page_count = doc.get_pages().len();

    // Try to extract document info dictionary
    let mut metadata = PdfMetadata {
        page_count,
        file_size,
        ..Default::default()
    };

    // Extract info from document trailer if available
    if let Ok(info_dict) = doc.trailer.get(b"Info") {
        if let Ok(info_ref) = info_dict.as_reference() {
            if let Ok(info) = doc.get_dictionary(info_ref) {
                metadata.title = extract_string_from_dict(info, b"Title");
                metadata.author = extract_string_from_dict(info, b"Author");
                metadata.subject = extract_string_from_dict(info, b"Subject");
                metadata.creator = extract_string_from_dict(info, b"Creator");
            }
        }
    }

    debug!(?metadata, "Metadata extraction complete");
    Ok(metadata)
}

/// Parse a PDF document, extracting both text and metadata.
///
/// This is the main entry point for PDF parsing.
///
/// # Arguments
/// * `path` - Path to the PDF file
///
/// # Returns
/// A complete `PdfDocument` with text, pages, and metadata.
#[instrument(level = "info", skip_all, fields(path = %path.as_ref().display()))]
pub fn parse_document<P: AsRef<Path>>(path: P) -> IngestionResult<PdfDocument> {
    let path = path.as_ref();

    let metadata = extract_metadata(path)?;
    let full_text = extract_text(path)?;
    let pages = extract_pages(path)?;

    Ok(PdfDocument {
        source_path: path.display().to_string(),
        metadata,
        full_text,
        pages,
    })
}

/// Helper to extract a string value from a lopdf dictionary.
fn extract_string_from_dict(
    dict: &lopdf::Dictionary,
    key: &[u8],
) -> Option<String> {
    dict.get(key)
        .ok()
        .and_then(|obj| {
            match obj {
                lopdf::Object::String(bytes, _) => {
                    // Try UTF-8 first, then Latin-1
                    String::from_utf8(bytes.clone())
                        .ok()
                        .or_else(|| Some(bytes.iter().map(|&b| b as char).collect()))
                }
                _ => None,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_extract_text_file_not_found() {
        let path = PathBuf::from("/nonexistent/file.pdf");
        let result = extract_text(&path);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IngestionError::FileNotFound(_)));
    }

    #[test]
    fn test_extract_metadata_file_not_found() {
        let path = PathBuf::from("/nonexistent/file.pdf");
        let result = extract_metadata(&path);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), IngestionError::FileNotFound(_)));
    }
}
