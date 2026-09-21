# Project instructions

- Windows 11, Rust stable MSVC. Preserve allocation-free successful report processing.
- User preference: always push completed work to GitHub and publish a GitHub release with the Windows binaries. Include honest validation and compatibility limits in the release notes.
- Before publishing: `cargo fmt --all --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, and `cargo build --locked --workspace --release`.
- Build the workspace before testing DLL integration. Set `OTD_TEST_PLUGIN` to the absolute path of `target/release/otd_ema_filter.dll` and run `cargo test --locked native_plugin_round_trip -- --ignored`.
- Run `gitleaks` on staged changes before committing configuration changes. Use the pinned source links in docs for upstream behavior. Do not claim live pen validation from replay or UI tests.
- Run `actionlint` for workflow edits and PowerShell ScriptAnalyzer for changed PowerShell scripts.
