# Philippine Labor Law Data Sources

This document catalogs the priority sources for the open-labor-ph dataset.

## Priority DOLE Department Orders

### Tier 1 (High Priority)

Most frequently referenced in HR/employment scenarios:

1. **DO-174-17**: Rules Implementing Articles 106 to 109 of the Labor Code, as Amended (contracting/subcontracting)
   - Source: [DOLE Official](https://www.dole.gov.ph/news/department-order-no-174-17-rules-implementing-articles-106-to-109-of-the-labor-code-as-amended/)
   - Status: 🟢 Processed (`data/processed/DO-174-17.json`, 37 sections) — OCR'd from official scan
2. **DO-202-19**: Implementing Rules and Regulations of RA 11165 (Telecommuting Act)

   - Source: [DOLE](https://www.dole.gov.ph/) (retrieved via Wayback snapshot, see `data/raw/manifest.json`)
   - Status: 🟢 Processed (`data/processed/DO-202-19.json`, 13 sections) — OCR'd from official scan
   - Note: Highly relevant post-COVID. Earlier revisions of this list cited a
     "DO-219-21" for telecommuting; DO-219 is actually a 2020 TUPAD order and
     the telecommuting IRR is DO-202-19.

3. **DO-147-15**: Amending the Implementing Rules and Regulations of Book VI of the Labor Code (termination of employment)

   - Source: [DOLE](https://www.dole.gov.ph/)
   - Status: 🔴 Not yet processed

4. **DO-18-A-11**: Rules Implementing Articles 106 to 109 of the Labor Code (contracting/subcontracting; superseded by DO-174-17)

   - Source: [DOLE](https://www.dole.gov.ph/)
   - Status: 🔴 Not yet processed

5. **DO-198-18**: Implementing Rules and Regulations of RA 11058 (Occupational Safety and Health Standards Law)
   - Source: [DOLE](https://www.dole.gov.ph/) (retrieved via Wayback snapshot, see `data/raw/manifest.json`)
   - Status: 🟢 Processed (`data/processed/DO-198-18.json`, 34 sections) — OCR'd from official scan

### Tier 2 (Medium Priority)

Important for specific industries or scenarios:

6. **DO-183-17**: Guidelines on the Implementation of Fixed-Term Employment

   - Status: 🔴 Not yet processed

7. **DO-57-16**: Guidelines on the Conduct of Drug Testing in the Workplace

   - Status: 🔴 Not yet processed

8. **DO-131-13**: Guidelines on Electronic Time Keeping and Electronic Payroll Systems

   - Status: 🔴 Not yet processed

9. **DO-190-18**: Guidelines on the Implementation of the Supreme Court Decision in G.R. No 202468 (Striking labor cases)

   - Status: 🔴 Not yet processed

10. **DO-169-17**: Rules on the Disposition of Monetary Claims of Overseas Filipino Workers
    - Status: 🔴 Not yet processed

## Labor Code Sections

### Book I: Pre-Employment (Articles 12-39)

- **Priority Articles**:
  - Art. 12-20: Recruitment and placement
  - Art. 21-39: Regulation of recruitment activities
- Status: 🟢 Processed (in `data/processed/labor_code.json`)

### Book II: Human Resources Development (Articles 40-80)

- **Priority Articles**:
  - Art. 40-59: Training and employment of special workers
  - Art. 60-80: Apprenticeship
- Status: 🟢 Processed (in `data/processed/labor_code.json`)

### Book III: Conditions of Employment (Articles 81-141)

- **Priority Articles** (HIGH PRIORITY):
  - Art. 82-96: Working conditions
  - Art. 97-111: Wages
  - Art. 112-128: Holiday pay, service charges
  - Art. 129-141: Wage administration
- Status: 🟢 Processed (in `data/processed/labor_code.json`)

### Book IV: Health, Safety and Social Welfare Benefits (Articles 142-175)

- **Priority Articles**:
  - Art. 142-162: Occupational health and safety
  - Art. 163-175: Social welfare benefits
- Status: 🟢 Processed (in `data/processed/labor_code.json`)

### Book V: Labor Relations (Articles 212-302)

- **Priority Articles** (HIGH PRIORITY):
  - Art. 212-221: General provisions
  - Art. 222-237: Unfair labor practices
  - Art. 263-276: Strikes and lockouts
- Status: 🟢 Processed (in `data/processed/labor_code.json`)

### Book VI: Post-Employment (Articles 277-302)

- **Priority Articles** (HIGHEST PRIORITY):
  - **Art. 279-286**: Termination of employment (most queried!)
  - Art. 287-291: Retirement
  - Art. 292-301: Retirement benefits
- Status: 🟢 Processed (in `data/processed/labor_code.json`)

## Additional Sources to Consider

### Republic Acts

- RA 6715: Security of Tenure amendments
- RA 10361: Domestic Workers Act (Batas Kasambahay)
- RA 11058: Occupational Safety and Health Law
- RA 11210: Expanded Maternity Leave Law
- RA 11199: Social Security Act amendments

### IRRs (Implementing Rules and Regulations)

- IRR of RA 11210 (Maternity Leave)
- IRR of RA 10361 (Kasambahay)

## Notes on Data Collection

- **PDF Availability**: Official DOLE uploads are predominantly *scanned image
  PDFs with no text layer* — this is the rule, not the exception. Every
  document acquired so far (DO-174-17, DO-198-18, DO-202-19, and both the
  2015 and 2017 renumbered Labor Code editions) is a scan requiring OCR.
- **Site Access**: `dole.gov.ph` and its bureau/regional subdomains serve a
  Cloudflare browser challenge to non-browser clients. Acquisition uses
  Wayback Machine snapshots (`id_` raw captures) of the official URLs; both
  the official URL and the snapshot URL are recorded per document in
  `data/raw/manifest.json`. Beware truncated captures — verify file
  integrity (`startxref` present, hash recorded) before trusting a snapshot.
- **Official Sources**: Prioritize Official Gazette and DOLE official website
- **Amendments**: Track which orders supersede previous versions
- **Metadata Tracking**: Record crawl date, source URL, and file hash for provenance in `data/raw/manifest.json`

## Labor Code Source

- **Department Advisory No. 1, Series of 2015** (Labor Code of the
  Philippines, Renumbered) is the canonical renumbered edition.
- Status: 🟢 Processed (`data/processed/labor_code.json`, 317 articles, Books I–VII) — OCR'd from official scan

## Status Legend

- 🔴 Not yet processed
- 🟡 In progress
- 🟢 Completed and validated
