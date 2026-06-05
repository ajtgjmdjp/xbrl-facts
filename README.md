# xbrl-facts

Rust workspace for parsing XBRL instance documents into provenance-rich financial facts.

## Status

This project is early. The current parser supports a minimal XBRL 2.1 instance subset:

- `schemaRef`
- `context` with entity identifier, instant or duration period, and explicit dimensions
- typed dimensions with raw inner XML preservation
- `unit` with simple measures and divide numerator/denominator measures
- root-level facts with `contextRef`, `unitRef`, `decimals`, `precision`, `xml:lang`, and `xsi:nil`
- basic fact footnotes from `link:loc`, `link:footnoteArc`, and `link:footnote`
- normalization from raw facts into context/unit-resolved facts

It also supports a minimal XML/XHTML iXBRL subset:

- `ix:nonFraction` and `ix:nonNumeric`
- `ix:hidden` detection via `InlineMeta.is_hidden`
- inline fact `name`, `contextRef`, `unitRef`, `decimals`, `precision`, `scale`, `sign`, `target`, and `continuedAt` metadata
- `ix:continuation` text chaining for split inline facts
- normalized numeric parsing with comma removal plus `scale` and negative `sign` application
- basic numeric formats: `numcommadecimal`, zero dash, and parenthesized negatives

Full iXBRL transform registry handling and taxonomy labels from linkbases are still planned work.

## Crates

- `xbrl-facts-core`: parser, data model, and normalization pipeline
- `xbrl-facts-cli`: command-line interface

## CLI

```bash
cargo run --bin xbrl-facts -- parse tests/fixtures/minimal.xbrl --format json
cargo run --bin xbrl-facts -- parse tests/fixtures/minimal.xbrl --format jsonl
cargo run --bin xbrl-facts -- parse tests/fixtures/minimal.xbrl --format jsonl --facts normalized
cargo run --bin xbrl-facts -- parse tests/fixtures/inline.xhtml --format jsonl --facts normalized
cargo run --bin xbrl-facts -- inspect facts.jsonl --concept NetSales
```

`parse --format json` writes the full parsed instance document. `parse --format jsonl` writes facts, one per line. The default is raw facts; `--facts normalized` writes context/unit-resolved facts. `inspect` reads raw-fact JSONL output and optionally filters by concept local name.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo package --workspace --locked --allow-dirty
```
