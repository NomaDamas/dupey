# Legacy HWP 3.0 extraction

## Problem

Files beginning with `HWP Document File V3.00` were passed to the HWP 5.x
CFB reader. This produced `wrong magic number: [48, 57, 50, 20, 44, 6f, 63, 75]`
before any document text could be read. These bytes identify a different file
format, not a damaged CFB header.

## Implementation

- Detect the legacy signature before opening CFB. Keep the shared `Format::Hwp`
  and existing hashing, MinHash, and family scoring paths.
- Parse document information, optional raw-DEFLATE compression, seven font
  tables, styles, and recursive paragraph lists using checked byte reads.
- Distinguish logical hchar counts from physical control payload sizes. Decode
  HWP's 16-bit Korean encoding independently from HWP 5's UTF-16.
- Preserve table and empty-cell boundaries; recursively extract textboxes,
  headers, footers, notes, and picture captions. Read text in grouped drawings,
  whose declared object counts exclude grouping containers.
- Extract date fields, automatic numbers, cross-reference display values,
  overstruck characters, and mail-merge names. Use explicit normalized outline
  notation that retains numbering selectors, custom bullets, and decorations.
- Return errors for truncated data, malformed controls, unsupported characters,
  excessive nesting, or password protection. Never accept a valid prefix as a
  complete document. Image data and layout remain outside text comparison.

## References and verification approach

The binary layout follows Hancom's *HWP 3.0 / HWPML revision 1.2* specification.
Korean character mappings are cross-checked against standard Johab/KS X 1001
and rhwp's HWP-specific mappings; adapted mappings retain rhwp's MIT notice.

Self-contained synthetic fixtures exercise the public extractor and CLI,
including Korean Windows paths, compression, modified dates/cells, nested
content, drawing groups, short cross-reference targets, and every truncation
of a small document. Public rhwp sample documents provide independent real-file
compatibility checks; those documents are not redistributed in this repository.
