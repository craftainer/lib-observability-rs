#!/usr/bin/env bash
# Pre-fetches the locked dependency graph so the first build/test works
# offline-ish and fails early if Cargo.lock has drifted from Cargo.toml.
set -euo pipefail

cargo fetch --locked
