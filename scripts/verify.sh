#!/usr/bin/env bash
# Requires a Rust toolchain and network access for initial dependency resolution.
# No real model endpoint, credentials, or paid API are used by this script.
set -Eeuo pipefail
native_tauri=false
if [ "${1:-}" = "--native-tauri" ]; then native_tauri=true; fi
cd "$(dirname "$0")/.."
mkdir -p verification
exec > >(tee verification/verify.log) 2>&1
finish() {
  code=$?
  if [ "$code" -eq 0 ]; then
    printf '{"status":"passed","scope":"rust-workspace-default-features,js-clients,offline-demos","live_model_tested":false}\n' > verification/status.json
  else
    printf '{"status":"failed","exit_code":%s,"live_model_tested":false}\n' "$code" > verification/status.json
  fi
}
trap finish EXIT
printf 'Verification started: '; date -u
command -v node
node scripts/test-clients.mjs
command -v cargo
command -v rustc
rustc -Vv | tee verification/rustc-version.txt
cargo -V | tee verification/cargo-version.txt
if [ ! -f Cargo.lock ]; then cargo generate-lockfile; fi
cargo fmt --all
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
# Clippy warnings are reported, not silently treated as passed production review.
cargo clippy --workspace --all-targets --locked
cargo run --locked -p agent-demo --bin minimal
cargo run --locked -p agent-demo --bin memory
cargo run --locked -p agent-demo --bin planner
cargo run --locked -p agent-demo --bin generic_extensions
if [ "$native_tauri" = true ]; then
  cargo check -p tauri-plugin-agent-bridge --features tauri --all-targets --locked
  cargo check -p agent-tauri-composition --features tauri --locked
fi
printf '%s\n' "$native_tauri" > verification/native-tauri-requested.txt
printf 'Verification finished: '; date -u
printf 'Retain Cargo.lock and the toolchain version above. Review warnings and run live-provider acceptance separately.\n'
