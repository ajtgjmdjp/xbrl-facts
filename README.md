# xbrl-facts

A Rust iXBRL / XBRL 2.1 parsing engine that turns regulatory filings into
provenance-rich, AI-ready financial facts. First-class support for SEC EDGAR
(US 10-K / 10-Q) and EDINET (Japan 有報) filings.

## Why

XBRL is the global standard for filing structured financial data, but the
toolchain is fragmented: SEC consumers reach for Arelle (Python, slow on
batch), Japanese researchers reach for `edinet-xbrl` (Python, EDINET-only),
and AI/LLM pipelines build one-off scrapers that re-implement the basics.

`xbrl-facts` is one engine that:

- **Speaks both dialects out of the box.** The same binary parses Apple's
  10-K and トヨタ自動車's 有報 into the same normalized fact stream.
- **Preserves provenance.** Every normalized fact carries the byte range of
  its source element, the original `contextRef`, and the doc id — so an
  LLM can cite the exact span it pulled a number from.
- **Stays lossless.** The parser emits a faithful `RawFact` first; the
  `NormalizedFact` (with applied scale, transforms, and resolved labels)
  is a derived view. Nothing about the original filing is discarded.
- **Is liberally licensed.** Apache-2.0 / MIT — usable inside commercial
  products, unlike AGPL alternatives.

## Status

Working today, exercised by integration tests against real filings:

- ✅ XBRL 2.1 instance (contexts, units, segments/scenarios, dimensions)
- ✅ Inline XBRL 1.1 (`ix:nonFraction`, `ix:nonNumeric`, `ix:hidden`,
  nested facts inside text blocks, `ix:continuation`)
- ✅ Inline XBRL Document Sets (IXDS) — merge a directory of EDINET
  `*_ixbrl.htm` files into one virtual instance
- ✅ Numeric transforms: `num-dot-decimal`, `num-comma-decimal`,
  `zerodash`, `numdash`, `numwordsen` (SEC English number words),
  parenthesized negatives, `scale`/`sign`
- ✅ Date transforms: `dateyearmonthdaycjk` and friends (Japanese
  full-width and CJK numerals), `dateerayearmonthdayjp` (令和/平成/昭和/…
  Gregorian conversion), `dateyearmonthdayen`-family for SEC
- ✅ Label linkbase resolution (concept QName → 日本語/English label)
- ✅ Byte-range provenance on every fact

## Performance

Single-threaded `--release` build on Apple M-series, parsing real filings
end-to-end (XML → `InstanceDocument` with all facts/contexts/units):

| Workload                                | Throughput | Per filing |
| --------------------------------------- | ---------- | ---------- |
| SEC Apple FY25 10-K (1.5 MB, 1131 facts)| 60 MB/s    | 23 ms      |
| EDINET 有報 (497 KB IXDS, 277 facts)    | 86 MB/s    | 5.6 ms     |
| 10 diverse EDINET 有報 (36 MB total)    | 100 MB/s   | 36 ms      |

The last row is 10 actual filings pulled from EDINET for FY25 — covering
製糖, 持株会社, 宇宙ベンチャー, アセットマネジメント, 信託銀行, etc. —
all parsed successfully (`Summary: 10 OK, 0 failed, 12 743 facts in 0.36 s`).
At 28 filings/s, EDINET's full annual flow (~50 000 filings) finishes in
about 30 minutes on a single thread.

Reproduce locally with:

```bash
cargo run --release --example bench_throughput
cargo run --release --example bench_real_edinet -- /path/to/extracted/filings
```

Planned:

- `<ix:tuple>` support (rare in EDINET/SEC but spec-required)
- Bundled standard taxonomy labels so SEC `us-gaap:` and EDINET
  `jpcrp_cor:` concepts resolve out of the box
- PyO3 Python bindings (`pyfacts` on PyPI)

## Crates

- [`xbrl-facts-core`](crates/xbrl-facts-core) — parser, data model,
  normalization, linkbase resolver
- [`xbrl-facts-cli`](crates/xbrl-facts-cli) — `xbrl-facts` command-line tool

## CLI

```bash
# Parse a single instance and print the full document as JSON
xbrl-facts parse tests/fixtures/minimal.xbrl --format json

# Stream raw facts as JSONL
xbrl-facts parse tests/fixtures/minimal.xbrl --format jsonl

# Stream normalized facts (decimal/period/entity resolved)
xbrl-facts parse tests/fixtures/minimal.xbrl --format jsonl --facts normalized

# Parse a whole EDINET filing directory as a single IXDS
xbrl-facts parse tests/fixtures/edinet/nippon-beet-sugar-fy2025 \
    --format jsonl --facts normalized

# Parse Apple's 10-K (single-file iXBRL)
xbrl-facts parse tests/fixtures/sec/aapl-10k-fy25.htm \
    --format jsonl --facts normalized

# Add label resolution for filer-specific concepts
xbrl-facts parse tests/fixtures/edinet/nippon-beet-sugar-fy2025 \
    --facts normalized \
    --schema tests/fixtures/edinet/nippon-beet-sugar-fy2025/jpcrp030000-asr-001_E00355-000_2025-03-31_01_2025-06-26.xsd \
    --labels tests/fixtures/edinet/nippon-beet-sugar-fy2025/jpcrp030000-asr-001_E00355-000_2025-03-31_01_2025-06-26_lab.xml \
    --lang ja

# Filter previously-saved JSONL by concept name
xbrl-facts inspect facts.jsonl --concept NetSalesSummaryOfBusinessResults
```

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo package --workspace --locked --allow-dirty
```

## License

Apache License, Version 2.0 ([LICENSE](LICENSE) or
<http://www.apache.org/licenses/LICENSE-2.0>).
