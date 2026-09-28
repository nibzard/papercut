# Release rules

Short rules for cutting a papercut release. Keep this file in step with practice.

1. **Gate before every release.** Clean tree, `cargo test` green,
   `cargo clippy --all-targets -- -D warnings` clean, `cargo publish --dry-run`
   passes. Do not release a dirty tree and do not bypass hooks.
2. **Package name is `papercut-cli`.** The binary is `papercut`. The `papercut`
   name on crates.io belongs to an unrelated crate; do not try to take it.
3. **License metadata is `MIT` only.** The repo ships one MIT LICENSE file.
   Do not write `MIT OR Apache-2.0`.
4. **Order:** set version in `Cargo.toml` → commit → annotated tag `vX.Y.Z` on
   that commit → push `main` and the tag → `cargo publish` →
   `gh release create vX.Y.Z` with short notes (install command, highlights).
5. **Tag the commit you published.** The tag, the crates.io version, and the
   `Cargo.toml` version must match.
6. **Account setup, once per machine:** `cargo login` with a crates.io token
   (publish-new scope) and a verified email address on crates.io. Both must be
   in place before `cargo publish` can succeed.
