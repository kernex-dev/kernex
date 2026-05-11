# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.2](https://github.com/kernex-dev/kernex/compare/v0.6.1...v0.6.2) - 2026-05-11

### Other

- *(memory)* migrations fast-path + memory-typed-row-shape change doc

## [0.6.1](https://github.com/kernex-dev/kernex/compare/v0.6.0...v0.6.1) - 2026-05-10

### Other

- *(deps)* replace em-dash with period or colon in crate descriptions

## [0.6.0](https://github.com/kernex-dev/kernex/compare/v0.5.0...v0.6.0) - 2026-05-10

### Added

- *(memory)* introduce MemoryStore trait + soft-delete on facts + Runtime::store_handle()

### Other

- *(workspace)* relocate inner attributes above crate doc blocks; tighten kernex-presets forward-compat
- rewrite workspace-profile-baseline SDD as a static change record
- *(deps)* drop unused dependencies flagged by cargo-machete
