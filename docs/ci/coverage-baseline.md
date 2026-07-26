# Coverage Baseline

CI runs the local coverage script:

```bash
scripts/coverage.sh 23
```

The current baseline is **23% line coverage**. The threshold is intentionally conservative because the project still contains OS/service/network integration paths that are not practical to execute in unit tests. Raise this threshold when new testable logic is added.

Coverage artifacts are written to:

- `target/coverage/tarpaulin-report.html`
- `target/coverage/cobertura.xml`

The GitHub Actions workflow uploads `target/coverage/` as a CI artifact for inspection.
