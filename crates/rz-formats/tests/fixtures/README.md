# Test fixtures

- `container.rar`: 87-byte RAR 4 archive with one file, `container` ("#!/bin/sh\nexit 0\n").
  Copied from the Homebrew test suite (`Library/Homebrew/test/support/fixtures/cask/container.rar`),
  BSD-2-Clause. It is the only RAR sample in the repository because RustZIP cannot create RAR files.
