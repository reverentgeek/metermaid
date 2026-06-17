## Summary

<!-- What does this PR change, and why? -->

## Related issues

<!-- e.g. Closes #123 -->

## Checklist

- [ ] `pnpm build` passes
- [ ] `cargo fmt --check` passes (run `cargo fmt`)
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] Added/updated tests for behavior changes (audio/analysis changes especially)
- [ ] Realtime audio callback remains allocation- and lock-free (if touched)

## Notes for reviewers

<!-- Anything that needs manual verification, e.g. tested with a specific device -->
