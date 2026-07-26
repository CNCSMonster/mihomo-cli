# E2E test framework

The E2E tests live under `tests/e2e/` and are wired into Cargo through
`tests/e2e.rs`.

Current covered flow:

1. create an isolated fixture config directory with an active subscription;
2. run a real `mihomo-cli` command that merges a user rule into `config.yaml`;
3. provide `MIHOMO_CLI_MIHOMO_PATH` pointing to a fake mihomo executable that
   asserts the CLI invokes `mihomo -t -d <config-dir>` after the write;
4. parse the resulting `config.yaml` with `serde_yaml` and assert the merged
   rule is present.

Run the whole suite with:

```bash
cargo test
```
