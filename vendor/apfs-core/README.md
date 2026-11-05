# apfs-forensic

[![apfs-core](https://img.shields.io/crates/v/apfs-core.svg?label=apfs-core)](https://crates.io/crates/apfs-core)
[![apfs-forensic](https://img.shields.io/crates/v/apfs-forensic.svg?label=apfs-forensic)](https://crates.io/crates/apfs-forensic)
[![Docs.rs](https://img.shields.io/docsrs/apfs-forensic)](https://docs.rs/apfs-forensic)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE)
[![Sponsor](https://img.shields.io/badge/sponsor-h4x0r-ea4aaa?logo=github-sponsors)](https://github.com/sponsors/h4x0r)

[![CI](https://github.com/SecurityRonin/apfs-forensic/actions/workflows/ci.yml/badge.svg)](https://github.com/SecurityRonin/apfs-forensic/actions)
[![unsafe forbidden](https://img.shields.io/badge/unsafe-forbidden-success.svg)](https://github.com/rust-secure-code/safety-dance)
[![Security advisories](https://img.shields.io/badge/advisories-clean-success.svg)](deny.toml)

**A from-scratch APFS reader and a graded anomaly auditor — navigate Apple File
System containers, volumes, and snapshots by path, and surface the snapshot and
sealed-volume tampering, recoverable deleted records, object-map inconsistencies,
and encryption state that a "clean" macOS mount is built to hide.**

> **Status: design skeleton.** The module layout and public API reflect the
> design at [`docs/plans/2026-06-21-apfs-forensic-design.md`](docs/plans/2026-06-21-apfs-forensic-design.md);
> parser bodies are stubs pending implementation.

Two crates, one workspace:

- **[`apfs-core`](https://crates.io/crates/apfs-core)** — the reader: NXSB
  container + checkpoint ring, object map, B-trees, APSB volumes, file-system
  records (`j_key`), file extents, extended attributes, snapshots, the space
  manager, encryption-state, and transparent **decmpfs** decompression over any
  `Read + Seek` source. No `unsafe`, no C bindings. (Imports as `apfs_core`.)
- **[`apfs-forensic`](https://crates.io/crates/apfs-forensic)** — the auditor:
  turns parsed APFS structures into severity-graded
  [`forensicnomicon::report::Finding`](https://crates.io/crates/forensicnomicon)s,
  so an APFS volume's anomalies aggregate uniformly with the partition and
  container layers.

## Audit an APFS container

```toml
[dependencies]
apfs-forensic = "0.1"   # pulls in apfs-core
```

```rust
use apfs_core::ApfsContainer;
use apfs_forensic::{audit_container, Source};
use forensicnomicon::report::Observation;

let container = ApfsContainer::open(std::fs::File::open("disk.img")?)?;
let src = Source { analyzer: "apfs-forensic".into(), scope: "APFS".into(), version: None };

for anomaly in audit_container(&container) {
    let finding = anomaly.to_finding(src.clone());
    println!("[{:?}] {} — {}", finding.severity, finding.code, finding.note);
    // e.g. [Some(High)] APFS-SEALED-VOLUME-BROKEN — im_broken_xid set at xid …
}
# Ok::<(), apfs_core::ApfsError>(())
```

## Trust but verify

Panic-free (`unsafe_code = "forbid"`, bounds-checked readers, range-checked
length/offset/count fields, capped allocations, cycle-guarded tree walks),
fuzzed (one cargo-fuzz target per parsed structure + a full-pipeline target), and
validated against **real artifacts** — macOS itself (mount read-only and diff),
The Sleuth Kit `fsstat`/`fls`/`istat`, `fsapfsinfo` (libfsapfs), and `apfsck`
(apfsprogs). See [`docs/validation.md`](docs/validation.md).

---

[Privacy Policy](https://securityronin.github.io/apfs-forensic/privacy/) · [Terms of Service](https://securityronin.github.io/apfs-forensic/terms/) · © 2026 Security Ronin Ltd
