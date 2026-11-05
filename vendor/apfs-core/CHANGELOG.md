# Changelog

All notable changes to `apfs-core` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.6](https://github.com/SecurityRonin/apfs-forensic/compare/apfs-core-v0.2.5...apfs-core-v0.2.6) - 2026-07-22

### Added

- *(apfs)* GREEN — real unallocated-extent enumeration via spaceman

## [0.2.4](https://github.com/SecurityRonin/apfs-forensic/compare/apfs-core-v0.2.3...apfs-core-v0.2.4) - 2026-07-19

### Fixed

- *(deps)* bump forensic-vfs 0.4 -> 0.5

## [0.2.2]

### Changed

- Migrate to forensic-vfs 0.3 (FsKind newtype). The `vfs` adapter's `kind()` now
  returns the string-backed `FsKind::APFS` const (re-exported from
  `forensicnomicon-core`) instead of the removed `enum FsKind::Apfs` variant.
