# HWP exact-match correction

**Goal:** Preserve dates and table values in canonical HWP text, so edited
reports cannot become exact matches through incomplete extraction.

**Design:** Keep SHA-256 and shared similarity scoring unchanged. Correct
the HWP 5 record reader to use byte lengths, decode UTF-16 and control spans,
and propagate malformed-input errors instead of returning a partial body.
Keep section and paragraph order. Reject unsupported protected streams.
Align CLI empty-text comparison with scan's original-byte fallback.

## Implementation and verification

1. Correct fixture record lengths and add independently encoded regressions
   in `crates/dupey-core/src/extract/hwp.rs` and CLI integration tests. Cover
   dates/table edits, compression, extended records, Unicode, control data,
   section order, malformed input and empty-text comparisons. Observe failure.
2. Fix HWP extraction and CLI comparison; update the example fixture writer.
   Confirm identical extracted content still has the same exact hash.
3. Run `cargo fmt --all -- --check`,
   `cargo clippy --workspace --all-targets --all-features -- -D warnings`,
   `cargo test --workspace`, and the existing end-to-end fixture suite.
4. Document that HWP hashes must be regenerated and that exact means equality
   of extracted text, with byte comparison only for empty extraction.

## Reference

Hancom's [record parsing](https://tech.hancom.com/python-hwp-parsing-1/)
and [paragraph/control description](https://tech.hancom.com/python-hwp-parsing-2/).
The old parser and both fixture writers treated the 12-bit byte length as
dwords. The parser also silently stopped on truncated records, allowing a
common prefix or empty output to hide later edits.

## Results

- Before the fix, the CLI date-edit regression returned `exact_equal=true`;
  byte-sized record tests showed empty or corrupted text. The parser tests
  also reproduced loss of empty-cell positions and accepted truncated DEFLATE.
- After the fix: 98 workspace tests passed; formatting and Clippy passed.
  The release end-to-end suite passed all 22 checks (117 files, 7 families).
- Independently checked 34 unprotected files from the public
  [pyhwp fixtures](https://github.com/mete0r/pyhwp/tree/master/tests/hwp5_tests/fixtures).
  All parsed; 33 retained all 376 reference text chunks. `sample-5017.hwp`
  retains its legacy U+F53A glyph rather than pyhwp's decomposed old-Hangul
  spelling; its nested table values were also checked. Password and
  distribution samples produce explicit errors. No third-party fixtures
  were copied into this repository.
- User-reported original files were not available for direct verification.
  Regenerate previously stored HWP hashes using the corrected extractor.
