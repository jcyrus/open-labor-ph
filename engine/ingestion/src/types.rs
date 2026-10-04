use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Serde adapter for date fields whose JSON schemas declare
/// `format: "date"` (plain `YYYY-MM-DD`). The domain types keep the richer
/// `DateTime<Utc>`; serialization truncates to the calendar date and
/// deserialization accepts both the plain date and full RFC 3339 forms.
pub mod schema_date {
    use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        date: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&date.format("%Y-%m-%d").to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        let raw = String::deserialize(deserializer)?;
        parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(format!("invalid date (expected YYYY-MM-DD): {raw}"))
        })
    }

    pub(super) fn parse(raw: &str) -> Option<DateTime<Utc>> {
        if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
            return Some(dt.with_timezone(&Utc));
        }
        NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .ok()
            .map(|d| Utc.from_utc_datetime(&d.and_time(NaiveTime::MIN)))
    }
}

/// [`schema_date`] for `Option<DateTime<Utc>>` fields.
pub mod schema_date_opt {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        date: &Option<DateTime<Utc>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match date {
            Some(d) => super::schema_date::serialize(d, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DateTime<Utc>>, D::Error> {
        let raw = Option::<String>::deserialize(deserializer)?;
        match raw {
            None => Ok(None),
            Some(s) => super::schema_date::parse(&s).map(Some).ok_or_else(|| {
                serde::de::Error::custom(format!("invalid date (expected YYYY-MM-DD): {s}"))
            }),
        }
    }
}

/// Where a dataset record came from and what produced it. Deliberately
/// free of timestamps: re-parsing the same PDF with the same parser yields
/// byte-identical output, so dataset diffs show only real changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Lowercase hex SHA-256 of the source PDF
    pub source_sha256: String,

    /// Parser that produced the record, e.g. "labor-ingestion 0.1.0"
    pub parser: String,
}

impl Provenance {
    /// Provenance for a record parsed from a PDF with this hash by this
    /// crate's current version.
    pub fn new(source_sha256: String) -> Self {
        Provenance {
            source_sha256,
            parser: concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION")).to_string(),
        }
    }
}

/// Represents a DOLE Department Order
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoleOrder {
    /// Order number (e.g., "DO-202-19", or "DO-18-A-11" for lettered amendments)
    pub order_number: String,

    /// Full title of the order
    pub title: String,

    /// Date when the order becomes effective. `None` when the document does
    /// not determine it (e.g. "fifteen days after publication" with no known
    /// publication date) — never guessed.
    #[serde(
        default,
        with = "schema_date_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub effective_date: Option<DateTime<Utc>>,

    /// Official source URL
    pub source_url: String,

    /// Source PDF hash and parser version
    pub provenance: Provenance,

    /// Additional metadata
    #[serde(default)]
    pub metadata: OrderMetadata,

    /// Hierarchical sections
    pub sections: Vec<Section>,
}

/// Metadata for a DOLE Order
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrderMetadata {
    /// Issuing authority name and title
    #[serde(default = "default_issuing_authority")]
    pub issuing_authority: String,

    /// Previous orders that this supersedes
    #[serde(default)]
    pub supersedes: Vec<String>,

    /// Related laws and regulations
    #[serde(default)]
    pub related_laws: Vec<String>,

    /// Topical tags
    #[serde(default)]
    pub tags: Vec<String>,

    /// Publication date
    #[serde(
        default,
        with = "schema_date_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub published_date: Option<DateTime<Utc>>,

    /// Date the order was signed, from the signature block
    #[serde(
        default,
        with = "schema_date_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub signed_date: Option<DateTime<Utc>>,

    /// The sentence stating when the order takes effect, whitespace-normalized
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effectivity_clause: Option<String>,
}

fn default_issuing_authority() -> String {
    "Secretary of Labor and Employment".to_string()
}

/// A section within a DOLE Order
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    /// Section identifier (e.g., "Section 1", "Article III")
    pub section_number: String,

    /// Section title or heading
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Full text content
    pub content: String,

    /// Nested subsections
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subsections: Vec<Subsection>,
}

/// A subsection or paragraph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subsection {
    /// Subsection label (e.g., "a", "1", "i")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    /// Text content of this item only (nested items live in `subsections`)
    pub content: String,

    /// Nested items, e.g. "(1)" items under "(a)", or "(i)" under "(1)"
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subsections: Vec<Subsection>,
}

/// Represents an article from the Labor Code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaborCodeArticle {
    /// Article number in the renumbered Labor Code (DOLE, 2015)
    pub article_number: u32,

    /// Article number before the 2015 renumbering ("Art. 294 [279]" → 279)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub former_article_number: Option<u32>,

    /// Book classification
    pub book: LaborCodeBook,

    /// Title within the book
    pub title_name: String,

    /// Chapter (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,

    /// Article heading
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,

    /// Full text content
    pub content: String,

    /// Metadata
    #[serde(default)]
    pub metadata: LaborCodeMetadata,

    /// Source PDF hash and parser version
    pub provenance: Provenance,

    /// Subsections
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subsections: Vec<Subsection>,
}

/// Labor Code book classifications
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaborCodeBook {
    /// Articles 1–11, which precede Book One.
    #[serde(rename = "Preliminary Title")]
    Preliminary,

    #[serde(rename = "Book I: Pre-Employment")]
    BookI,

    #[serde(rename = "Book II: Human Resources Development")]
    BookII,

    #[serde(rename = "Book III: Conditions of Employment")]
    BookIII,

    #[serde(rename = "Book IV: Health, Safety and Social Welfare Benefits")]
    BookIV,

    #[serde(rename = "Book V: Labor Relations")]
    BookV,

    #[serde(rename = "Book VI: Post-Employment")]
    BookVI,

    /// Penal provisions, prescription of offenses and money claims, and
    /// transitory provisions (renumbered Arts. 303–317).
    #[serde(rename = "Book VII: Transitory and Final Provisions")]
    BookVII,
}

/// Metadata for Labor Code articles
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaborCodeMetadata {
    /// Whether this is from original PD 442
    #[serde(default = "default_true")]
    pub original_pd_442: bool,

    /// Amendments to this article
    #[serde(default)]
    pub amendments: Vec<Amendment>,

    /// Related article numbers
    #[serde(default)]
    pub related_articles: Vec<u32>,

    /// Implementing DOLE Orders. The Labor Code text does not cite them, so
    /// the PDF parser leaves this empty; it is filled by cross-linking with
    /// the DOLE Order dataset.
    #[serde(default)]
    pub implementing_orders: Vec<String>,

    /// Topical tags
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// An amendment to a Labor Code article
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Amendment {
    /// Law that made the amendment (e.g., "RA 6715")
    pub law: String,

    /// Effectivity date, when the amendment note states one
    #[serde(
        default,
        with = "schema_date_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub date: Option<DateTime<Utc>>,

    /// Description of the change
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A benchmark question for RAG evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkQuestion {
    /// Unique identifier
    pub id: String,

    /// Topic category
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<QuestionCategory>,

    /// Difficulty level
    #[serde(skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,

    /// The question text
    pub question: String,

    /// Expected answer
    pub expected_answer: String,

    /// Legal citations
    pub citations: Vec<String>,

    /// Tags for filtering
    #[serde(default)]
    pub tags: Vec<String>,

    /// Additional metadata
    #[serde(default)]
    pub metadata: QuestionMetadata,
}

/// Question categories
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionCategory {
    Termination,
    Probationary,
    Wages,
    Benefits,
    Leave,
    Osh,
    LaborRelations,
    Contracts,
    Telecommuting,
    Discrimination,
    Other,
}

/// Difficulty levels
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

/// Metadata for benchmark questions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuestionMetadata {
    /// Where the question originated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Whether RAG context is required
    #[serde(default = "default_true")]
    pub requires_context: bool,

    /// Common incorrect answers
    #[serde(default)]
    pub common_mistakes: Vec<String>,
}
