# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.0.2](https://github.com/ggagosh/arcula/compare/v2.0.1...v2.0.2) - 2026-05-11

### Changed

- Disabled `.env` loading by default so stored secure connections are the default source of connection configuration.
- Added the global `--env` flag to explicitly load `.env` for legacy, CI, migration, and one-off workflows.
- Updated the Arcula agent skill and documentation to use the safer default connection behavior.

## [2.0.1](https://github.com/ggagosh/arcula/compare/v2.0.0...v2.0.1) - 2026-05-05

### Added

- Added the `arcula-cli` agent skill for safe Arcula usage from AI coding agents.

### Changed

- Updated the README introduction and documented agent skill installation with `npx skills`.

## [2.0.0](https://github.com/ggagosh/arcula/compare/v1.0.6...v2.0.0) - 2026-05-05

### Added

- Added a secure connection manager backed by the OS credential store, with `connection add/list/show/test/remove/import-env` commands.
- Added environment metadata and per-connection safety policy support for `kind`, `protected`, source/target permissions, agent apply, human approval, destructive backup, and backup verification.
- Added agent-friendly output and execution controls: `--format json`, `--agent`, `--no-env`, `--no-color`, direct URI inputs, and URI kind overrides.
- Added saved sync plans via `sync plan`, including plan hashes, policy snapshots, warnings, approval requirements, and human-readable/JSON rendering.
- Added OS-backed approval records for protected plans via `plan approve`, with approval signatures bound to the exact plan hash.
- Added operation records via `operation run/list/show`, including status history, approval metadata, sync reports, and errors.
- Added operation revert support using the pre-sync target backup.

### Changed

- Destructive syncs to production/protected targets now require a full backup and, when policy requires it, the plan/approval/operation flow.
- Direct target URI usage is treated as protected by default unless classified with `--to-kind`.
- `info` now includes stored connections in addition to `.env` environments.
- Linux secure storage now uses the kernel keyring backend to avoid a DBus development dependency in CI and headless builds.

### Fixed

- Sync export/import failures now return errors instead of printing success.
- Required backup failures now abort the sync before destructive target changes.
- Failed imports attempt backup restoration but still return a failed operation.
- MongoDB command errors are sanitized before being stored or emitted.
- Docker integration tests now satisfy strict clippy checks.

### Breaking

- The public Rust API changed: `SyncParams` gained fields, `ConfigError` gained `ConnectionStore`, and the deprecated `commands::sync::execute` helper was removed.

## [1.0.6](https://github.com/ggagosh/arcula/compare/v1.0.5...v1.0.6) - 2025-12-24

### Other

- Fix --nsInclude usage
- Remove deprecated --db flag from mongorestore

## [1.0.5](https://github.com/ggagosh/arcula/compare/v1.0.4...v1.0.5) - 2025-05-19

### Other

- Update Cargo.toml with repository, categories, and keywords
- Add author note to README explaining project purpose
