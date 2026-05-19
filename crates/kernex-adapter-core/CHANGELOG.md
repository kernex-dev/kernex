# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.2](https://github.com/kernex-dev/kernex/compare/v0.8.1...v0.8.2) - 2026-05-19

### Added

- *(adapter-core)* add Detection::new public constructor (FU-E-01) ([#28](https://github.com/kernex-dev/kernex/pull/28))

### Added

- `Detection::new(installed, config_root, version)` public constructor (FU-E-01). Lets downstream consumers build the value without routing through `serde_json::from_value` while the struct remains `#[non_exhaustive]`. Wire format is unchanged; pinned by the new `detection_new_roundtrips` smoke test.
