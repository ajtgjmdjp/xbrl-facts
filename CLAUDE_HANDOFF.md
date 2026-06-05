# Claude Handoff - xbrl-facts

Date: 2026-05-24
Repo: `/Users/rei/Project/xbrl-facts`

## Current State

This repo started as a scaffold-only Rust workspace. It now has a working first-pass parser and CLI for a useful subset of XBRL/iXBRL.

Workspace crates:

- `xbrl-facts-core`: data model, parser, normalization
- `xbrl-facts-cli`: CLI binary installed as `xbrl-facts`

Everything is currently uncommitted.

## Implemented In This Session

Core parser and model:

- Replaced public `todo!()` parser/normalizer with real implementations.
- Added minimal XBRL 2.1 instance parsing:
  - `schemaRef`
  - `context`
  - entity identifier
  - instant/duration/forever periods
  - segment/scenario explicit dimensions
  - typed dimensions with raw inner XML preservation
  - `unit`, including divide numerator/denominator
  - root-level facts with `contextRef`, `unitRef`, `decimals`, `precision`, `xml:lang`, `xsi:nil`
  - basic fact footnotes via `link:loc`, `link:footnoteArc`, `link:footnote`
- Added minimal XML/XHTML iXBRL parsing:
  - `ix:nonFraction`
  - `ix:nonNumeric`
  - `ix:hidden` detection
  - inline metadata capture: `format`, `scale`, `sign`, `target`, `continuedAt`
  - `ix:continuation` text chaining
- Added normalization:
  - context/unit resolution
  - dimensions copied into normalized facts
  - decimal parsing
  - inline comma cleanup
  - `scale`
  - negative `sign`
  - `numcommadecimal`
  - zero dash
  - parenthesized negatives

API/model fixes:

- `QName` equality/order/hash now ignore lexical prefix and use namespace URI + local name.
- `NormalizedFact.dimensions` is now JSON-safe: `Vec<Dimension>` instead of `BTreeMap<QName, ...>`.
- `NormalizedValue::Numeric.decimals` keeps `Decimals`, preserving `INF`.
- Typed dimensions use `raw_xml` instead of lossy text.
- License metadata changed to `Apache-2.0` to match the single license file.

CLI:

- `parse` now reads actual input files.
- `parse --format json` outputs the full parsed `InstanceDocument`.
- `parse --format jsonl` outputs facts one per line.
- `parse --format jsonl --facts normalized` outputs normalized facts.
- `inspect` reads raw-fact JSONL and filters by `--concept`.
- Binary name is explicitly `xbrl-facts`.

Docs/CI/fixtures:

- Added `README.md`.
- Added `.github/workflows/ci.yml`.
- Added root fixtures under `tests/fixtures/`.
- Added package-local CLI fixtures under `crates/xbrl-facts-cli/tests/fixtures/`.
- Added CLI integration tests.

## Important Files

- `crates/xbrl-facts-core/src/parser.rs`
  - Main parser and normalization implementation.
  - This file grew large quickly; likely next refactor target.
- `crates/xbrl-facts-core/src/types.rs`
  - Public data model.
- `crates/xbrl-facts-cli/src/main.rs`
  - CLI commands and output modes.
- `crates/xbrl-facts-cli/tests/cli.rs`
  - End-to-end CLI tests.
- `tests/fixtures/`
  - Human-facing fixtures for README/dev usage.
- `crates/xbrl-facts-cli/tests/fixtures/`
  - Duplicated package-local fixtures so packaged CLI tests can run.

## Verification Already Run

All of these passed after the latest changes:

```bash
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo package --workspace --locked --allow-dirty
```

Manual CLI checks also passed:

```bash
cargo run -q --bin xbrl-facts -- parse tests/fixtures/minimal.xbrl --format json
cargo run -q --bin xbrl-facts -- parse tests/fixtures/advanced.xbrl --format jsonl --facts normalized
cargo run -q --bin xbrl-facts -- parse tests/fixtures/inline.xhtml --format jsonl --facts normalized
cargo run -q --bin xbrl-facts -- parse tests/fixtures/footnote.xbrl --format json
```

## Known Limitations

- No real EDINET/iXBRL sample has been tested yet. Local search found only synthetic fixtures.
- iXBRL transform registry is not fully implemented.
- Continuation resolution is text chaining only. It does not handle every edge of the iXBRL spec.
- Footnote parsing is basic and only covers the common locator/arc/resource pattern.
- Taxonomy/linkbase label resolution is not implemented.
- Inline `format` is only partially interpreted for numeric normalization.
- `parser.rs` is now too large and should be split before much more logic is added.
- No byte-range provenance yet.
- Context/unit validation is still permissive.

## Recommended Next Steps

1. Commit the current work or at least review the diff before adding more.
2. Fetch or locate a small real EDINET filing package and run:

   ```bash
   cargo run -q --bin xbrl-facts -- parse path/to/file.xhtml --format jsonl --facts normalized
   ```

3. Fix the first real-file parser failures.
4. Split `parser.rs` into modules:
   - `parser/mod.rs`
   - `parser/instance.rs`
   - `parser/inline.rs`
   - `parser/normalize.rs`
   - `parser/qname.rs`
5. Add real EDINET fixtures, preferably minimized/redacted if needed.
6. Add more iXBRL transform cases only after real samples show which ones matter.

## Current Git Notes

Inside `/Users/rei/Project/xbrl-facts`, expected uncommitted changes include:

- Modified:
  - `Cargo.lock`
  - `Cargo.toml`
  - `crates/xbrl-facts-cli/Cargo.toml`
  - `crates/xbrl-facts-cli/src/main.rs`
  - `crates/xbrl-facts-core/Cargo.toml`
  - `crates/xbrl-facts-core/src/error.rs`
  - `crates/xbrl-facts-core/src/lib.rs`
  - `crates/xbrl-facts-core/src/parser.rs`
  - `crates/xbrl-facts-core/src/types.rs`
- New:
  - `.github/workflows/ci.yml`
  - `README.md`
  - `CLAUDE_HANDOFF.md`
  - `crates/xbrl-facts-cli/tests/`
  - `tests/fixtures/`

Do not touch unrelated parent workspace changes in `/Users/rei/Project`; the root repo has unrelated dirty state.
