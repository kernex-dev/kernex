# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0](https://github.com/kernex-dev/kernex/compare/v0.8.3...v0.9.0) - 2026-06-12

### Added

- *(sandbox)* deny subprocess network egress by default with per-tool opt-in ([#55](https://github.com/kernex-dev/kernex/pull/55))
- *(sandbox)* enforce required sandboxing on spawns; warn once when unsandboxed ([#54](https://github.com/kernex-dev/kernex/pull/54))
- *(sandbox)* lock down $HOME writes and deny credential reads (D-13 b) ([#53](https://github.com/kernex-dev/kernex/pull/53))
- *(sandbox)* isolate subprocess environments from provider credentials ([#51](https://github.com/kernex-dev/kernex/pull/51))

## [0.6.0](https://github.com/kernex-dev/kernex/compare/v0.5.0...v0.6.0) - 2026-05-10

### Other

- *(workspace)* relocate inner attributes above crate doc blocks; tighten kernex-presets forward-compat
- *(lints)* enforce unsafe_code = "deny" workspace-wide
