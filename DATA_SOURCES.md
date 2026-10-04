# Philippine Labor Law Data Sources

This document catalogs the priority sources for the open-labor-ph dataset.

Every entry below was checked on 2026-10-04 against the DOLE Bureau of Working
Conditions issuance index, the Labor Law PH Library index, and secondary legal
commentary. Check the official text before you ingest anything: titles and dates here are
for orientation, not citation.

## Priority DOLE Department Orders

### Tier 1 (High Priority)

The orders HR and employment questions most often turn on:

1. **DO-147-15**: Amending the Implementing Rules and Regulations of Book VI of the Labor Code (termination of employment)
   - Issued: 2015-09-07
   - Covers: just and authorized causes (Arts. 297–299), two-notice rule, due process
   - Source: [DOLE BLR](https://blr.dole.gov.ph/2015/11/12/dole-clarifies-rules-on-termination-of-employment/)
   - Status: 🔴 Not yet processed

2. **DO-174-17**: Rules Implementing Articles 106 to 109 of the Labor Code, as Amended (contracting and subcontracting)
   - Issued: March 2017
   - Supersedes: DO-18-A-11
   - Note: Excludes BPO/KPO-type IT-enabled services
   - Source: [DOLE BWC issuances](https://bwc.dole.gov.ph/issuances/department-orders/)
   - Status: 🔴 Not yet processed

3. **DO-202-19**: Implementing Rules and Regulations of RA 11165, the Telecommuting Act
   - Signed: 2019-03-26 · Published: 2019-04-24 · Effective: 2019-05-10
   - Source: [DOLE BWC issuances](https://bwc.dole.gov.ph/issuances/department-orders/)
   - Status: 🔴 Not yet processed

4. **DO-252-25**: Revised Implementing Rules and Regulations of RA 11058 (Occupational Safety and Health Standards)
   - Supersedes: DO-198-18
   - Source: [DOLE BWC issuances](https://bwc.dole.gov.ph/issuances/department-orders/) (secondary summary: [L&E Global](https://leglobal.law/2025/06/24/philippines-stricter-rules-safer-workplaces-tightening-of-occupational-safety-and-health-standards-under-do-252-25-or-the-revised-implementing-rules-and-regulations-of-r-a-no-11058/))
   - Status: 🔴 Not yet processed

5. **DO-183-17**: Revised Rules on the Administration and Enforcement of Labor Laws Pursuant to Article 128 of the Labor Code, as Renumbered (labor inspections, visitorial powers)
   - Source: [DOLE BWC issuances](https://bwc.dole.gov.ph/issuances/department-orders/)
   - Status: 🔴 Not yet processed

### Tier 2 (Medium Priority)

6. **DO-53-03**: Guidelines for the Implementation of a Drug-Free Workplace Policies and Programs for the Private Sector
   - Source: [DOLE OSHC (PDF)](https://oshc.dole.gov.ph/wp-content/uploads/2020/09/Department-Order-No.-53-03.pdf)
   - Status: 🔴 Not yet processed

7. **DO-230-21**: Guidelines on Support for Workers in the Informal Economy under RA 11313 (Safe Spaces Act)
   - Source: [DOLE BWC issuances](https://bwc.dole.gov.ph/issuances/department-orders/)
   - Status: 🔴 Not yet processed

### Historical (ingest for amendment tracking, mark as superseded)

- **DO-18-A-11**: Rules Implementing Articles 106 to 109 of the Labor Code. Superseded by DO-174-17.
- **DO-198-18**: IRR of RA 11058 (OSH Law). Effective 2019-01-25. Superseded by DO-252-25.

### Removed from the previous list

The original list paired these numbers with topics they do not cover. They are
recorded here so the corrections can be traced:

| Number as listed | Topic claimed | Actual issuance |
|---|---|---|
| DO-174-17 | Prevention of sexual harassment | Contracting and subcontracting (kept above under its real topic) |
| DO-219-21 | Telecommuting | DO-219-20 is the TUPAD emergency employment program; telecommuting is DO-202-19 |
| DO-147-15 | Probationary employment in retail | Book VI termination rules (kept above under its real topic) |
| DO-18-A-11 | OSH standards | Contracting rules, superseded by DO-174-17 |
| DO-198-18 | Apprenticeship and learnership | IRR of RA 11058 (OSH), superseded by DO-252-25 |
| DO-183-17 | Fixed-term employment | Labor standards enforcement (kept above under its real topic) |
| DO-57-16 | Drug testing | DO-57-04 is labor standards enforcement guidelines; drug testing is DO-53-03 |
| DO-131-13 | Electronic timekeeping and payroll | Rules on the Labor Compliance System |
| DO-190-18 | Implementation of a Supreme Court decision | "Sa Pinas, Ikaw ang Ma'am at Sir" program |
| DO-169-17 | OFW monetary claims | IRR of RA 10789 (Racehorse Jockeys Retirement Act) |

Sexual harassment comes from statute, not a Department Order: see RA 7877 and RA 11313 below.
Apprenticeship falls under Labor Code Book Two. Fixed-term employment
comes from case law (*Brent School v. Zamora*, G.R. No. L-48494), not a Department Order.

## Labor Code Sections

Article numbers follow the **renumbered** Labor Code (DOLE, 2015). Former
numbers appear in brackets. `parse-labor-code` keeps the renumbered
article as `article_number` and records the former number as a
`formerly_art_N` tag.

### Preliminary Title (Arts. 1–11)

- Chapter I: General Provisions (Arts. 1–6); Chapter II: Emancipation of Tenants (Arts. 7–11)
- Status: 🔴 Not yet processed

### Book One: Pre-Employment (Arts. 12–42)

- Recruitment and placement, overseas employment, employment of non-resident aliens
- Status: 🔴 Not yet processed

### Book Two: Human Resources Development Program (Arts. 43–81)

- National manpower development, apprentices, learners, handicapped workers
- Status: 🔴 Not yet processed

### Book Three: Conditions of Employment (Arts. 82–161 [82–155])

- **Priority** (HIGH): working conditions and rest periods, wages, contracting (Arts. 106–109),
  visitorial and enforcement power (Art. 128), recovery of wages and simple money claims (Art. 129)
- Status: 🔴 Not yet processed

### Book Four: Health, Safety and Social Welfare Benefits (Arts. 162–217 [156–210])

- Medical, dental and occupational safety (from Art. 162 [156], First-Aid Treatment); employees' compensation; Medicare; adult education
- Status: 🔴 Not yet processed

### Book Five: Labor Relations (Arts. 218–292 [211–277])

- **Priority** (HIGH): declaration of policy (Art. 218 [211]), unfair labor practices, strikes and lockouts
- Status: 🔴 Not yet processed

### Book Six: Post-Employment (Arts. 293–302 [278–287])

- **Priority** (HIGHEST, the most-asked area):
  - Art. 294 [279]: Security of Tenure
  - Art. 297 [282]: Termination by Employer (just causes)
  - Art. 298 [283]: Closure of Establishment and Reduction of Personnel (authorized causes)
  - Art. 299 [284]: Disease as Ground for Termination
  - Art. 302 [287]: Retirement
- Status: 🔴 Not yet processed

### Book Seven: Transitory and Final Provisions (Arts. 303–317 [288–302])

- **Priority** (HIGH):
  - Art. 305 [290]: Offenses (3-year prescription)
  - Art. 306 [291]: Money Claims (3-year prescription)
  - Art. 307 [292]: Institution of Money Claims
- Status: 🔴 Not yet processed

## Additional Sources to Consider

### Republic Acts

- RA 6715: Herrera-Veloso Law (security of tenure, labor relations amendments)
- RA 7877: Anti-Sexual Harassment Act of 1995
- RA 10361: Domestic Workers Act (Batas Kasambahay)
- RA 11058: Occupational Safety and Health Law
- RA 11165: Telecommuting Act
- RA 11199: Social Security Act of 2018
- RA 11210: 105-Day Expanded Maternity Leave Law
- RA 11313: Safe Spaces Act

### IRRs (Implementing Rules and Regulations)

- IRR of RA 11210 (Maternity Leave)
- IRR of RA 10361 (Kasambahay)

## Notes on Data Collection

- **PDF Availability**: Some older Department Orders exist only as scanned PDFs and need OCR
- **Official Sources**: Prefer the Official Gazette and DOLE sites (dole.gov.ph, bwc.dole.gov.ph, blr.dole.gov.ph, oshc.dole.gov.ph)
- **Amendments**: Record which orders supersede earlier ones (see Historical above)
- **Metadata Tracking**: Every document is listed in [`data/sources.toml`](data/sources.toml) with its official URL and pinned SHA-256. Parsed records carry `provenance.source_sha256` and the parser version. No timestamps are stored, so re-parsing the same PDF gives byte-identical output
- **Automated Downloads**: dole.gov.ph hosts (including BWC, BLR and OSHC) sit behind a Cloudflare JavaScript challenge, so their PDFs are saved by hand and then verified by `fetch`. The renumbered Labor Code downloads automatically from ILO NATLEX

## Status Legend

- 🔴 Not yet processed
- 🟡 In progress
- 🟢 Completed and validated
