# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.13.1](https://github.com/kernex-dev/kernex/compare/v0.13.0...v0.13.1) - 2026-11-26

### Other

- *(sandbox)* parse KERNEX_REQUIRE_SANDBOX values without setting it

## [0.12.0](https://github.com/kernex-dev/kernex/compare/v0.11.0...v0.12.0) - 2026-11-25

### Added

- [**breaking**] run the agent CLI providers under the tool policy
- *(providers)* [**breaking**] confine the file tools to the workspace
- *(sandbox)* [**breaking**] deny by default in the macOS profile
- *(sandbox)* [**breaking**] enforce the path policy on Linux by granting only what is allowed
- *(sandbox)* [**breaking**] scope tool writes to the workspace through one path policy

### Fixed

- *(sandbox)* [**breaking**] check a search root and its exclusions against one policy
- *(sandbox)* ask whether a directory can be searched with AT_EACCESS only
- *(sandbox)* check that a closed directory is still the one looked at
- *(sandbox)* treat an unknown answer about a stop's directory as ours
- *(sandbox)* trust a stop only where this user cannot have caused it
- *(sandbox)* resolve each policy path once and trust only stops it cannot cause
- *(sandbox)* refuse a policy whose paths stop inside a write root
- *(sandbox)* refuse paths that cannot be resolved completely
- *(sandbox)* follow links past a loop again, pin what a `..` steps out of
- *(sandbox)* pin a file root's directory and links to every protected path
- *(sandbox)* split guards from pins, bound symlink resolution
- *(sandbox)* guard links on the way to a write root, keep root metadata writable
- *(sandbox)* keep write roots from being swapped for a symlink
- *(providers)* [**breaking**] give OpenCode new directories for every call
- *(sandbox)* [**breaking**] stop treating ~/.local/share as a write root
- *(providers)* refuse relative MCP commands and undeclared servers for CLI providers
- *(providers)* match file rules on the resolved path and filter search results
- *(sandbox)* let Linux subprocesses list directories above a protected path
- *(providers)* keep skills readable and bound what the read tool opens

### Other

- describe the 0.12.0 sandbox, tools and memory as they now work
- declare Rust 1.86 as the minimum and check it in CI
- *(sandbox)* reopen the closed directory before asserting on it
- *(sandbox)* resolve paths in one pass from the resolved prefix

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
