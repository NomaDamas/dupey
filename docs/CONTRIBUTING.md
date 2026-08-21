# Contributing

## Rust checks

Install the repository hooks once after cloning:

```sh
pre-commit install
```

The pre-commit hook runs the same checks required for a Rust change:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

To format the workspace before committing:

```sh
cargo fmt --all
```

The hook does not rewrite files. A commit is rejected when formatting, lint,
or tests fail, so run the formatter locally and review the resulting diff
before retrying.
