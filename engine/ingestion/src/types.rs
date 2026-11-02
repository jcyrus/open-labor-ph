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

    /// The order that replaced this one, when the source manifest records it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,

    /// The text came from OCR of a scan, so it can contain reading errors
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ocr_text: bool,
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

    /// Implementing DOLE Orders cited in the article's editorial footnotes
    /// ("Refer to D.O. No. 147-15"), as normalized order numbers.
    /// Cross-linking with the DOLE Order dataset can add more.
    #[serde(default)]
    pub implementing_orders: Vec<String>,

    /// Editorial footnotes the source edition attaches to this article
    /// (amendment history, superseding laws, implementing rules)
    #[serde(default)]
    pub footnotes: Vec<Footnote>,

    /// Editorial footnotes printed on the Title or Chapter heading above
    /// this article; a repeal stated there covers every article under it
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heading_footnotes: Vec<HeadingFootnote>,

    /// Whether the source edition's editorial notes say this text no longer
    /// applies as printed. `None` when the notes of the parsed PDF have not
    /// been reviewed in `data/labor_code_status.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legal_status: Option<LegalStatus>,

    /// Topical tags
    #[serde(default)]
    pub tags: Vec<String>,
}

/// An editorial footnote from the source edition of the Labor Code
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footnote {
    /// Footnote number as printed
    pub number: u32,

    /// Footnote text, whitespace-normalized
    pub text: String,
}

/// An editorial footnote printed on a Title or Chapter heading
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadingFootnote {
    /// The heading carrying the footnote, as in `title_name` or `chapter`
    pub heading: String,

    /// Footnote number as printed
    pub number: u32,

    /// Footnote text, whitespace-normalized
    pub text: String,
}

/// What the source edition's editorial notes say about an article's currency.
/// This reports those notes; it is not a finding that an article is in force.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegalStatus {
    /// Overall reading of the notes
    pub status: LegalStatusKind,

    /// The notes behind `status`; empty for `no_change_noted`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<LegalChange>,
}

/// Overall status of an article, as far as the source edition's notes go
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegalStatusKind {
    /// No reviewed note says the text changed. Later laws may still apply.
    NoChangeNoted,

    /// Part of the article, or how it operates, no longer applies as printed
    PartlySuperseded,

    /// The whole article no longer applies as printed
    Superseded,
}

/// One reviewed editorial note saying an article no longer applies as printed
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegalChange {
    /// How the note describes the change
    pub effect: LegalEffect,

    /// The part affected, e.g. "second paragraph"; `None` for the whole article
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,

    /// Law or decision that made the change, e.g. "RA 11210"
    pub by: String,

    /// Number of the footnote stating the change
    pub footnote: u32,
}

/// How an editorial note describes a change
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegalEffect {
    Superseded,
    Repealed,
    Inoperative,
    Obsolete,
    Unconstitutional,
    FunctionsTransferred,
    Modified,
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

    /// Legal citations, each the id of a corpus passage
    pub citations: Vec<String>,

    /// Corpus text that supports the answer
    pub evidence: Vec<Evidence>,

    /// Facts an answer must state to be correct
    pub key_points: Vec<KeyPoint>,

    /// Tags for filtering
    #[serde(default)]
    pub tags: Vec<String>,

    /// Additional metadata
    #[serde(default)]
    pub metadata: QuestionMetadata,
}

/// A verbatim quote from a cited passage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    /// One of the question's citations
    pub citation: String,

    /// Text that must appear in that passage, ignoring case and whitespace
    pub quote: String,
}

/// One fact an answer must state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPoint {
    /// What the fact is
    pub label: String,

    /// Phrasings that count as stating it
    pub any_of: Vec<String>,
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
