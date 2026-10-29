# Vendored `pbz2`

- Upstream: [`soof-golan/pbz2`](https://github.com/soof-golan/pbz2), crates.io version `0.2.0`.
- License: MIT. Preserve both `LICENSE` and `NOTICE`; the notice includes attribution and terms for the bzip2-derived material used by `pbz2-core`.
- Local changes: add fallible worker-pool startup for the decoder; convert scanner thread creation failures into reader errors; contain worker/scanner panics so the caller receives a decode error instead of a disconnected response channel.
- Keep the upstream source changes small and review each upstream update against these error-handling changes.
