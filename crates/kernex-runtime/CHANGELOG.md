# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0](https://github.com/kernex-dev/kernex/compare/v0.5.0...v0.6.0) - 2026-05-10

### Added

- *(memory)* introduce MemoryStore trait + soft-delete on facts + Runtime::store_handle()
- *(workspace)* split workspace into kernex-adapter-core, kernex-presets, kernex-brain

### Other

- *(workspace)* relocate inner attributes above crate doc blocks; tighten kernex-presets forward-compat
- *(deps)* drop unused dependencies flagged by cargo-machete
