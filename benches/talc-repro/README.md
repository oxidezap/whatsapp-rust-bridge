# The talc bugs, as tests

This bridge used talc as its `#[global_allocator]` and dropped it in June 2026,
because talc 5.0.3 had two bugs that this workload reached in production. Both
were fixed in 5.0.4 and both are pinned here, so the next person who wants to
reopen the question starts from a red/green run instead of a changelog entry.

It is a standalone crate on purpose: showing a fix means running the same test
bodies against two versions of talc, and the bridge's own manifest pins one.

```
# green, the version the bridge would take
cargo update -p talc --precise 5.1.0
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  WASM_BINDGEN_TEST_ONLY_NODE=1 cargo test --target wasm32-unknown-unknown

# red, the version that broke
cargo update -p talc --precise 5.0.3
```

Run each test on its own (`-- --exact <name>`). The tests share one wasm
instance and one linear memory, so a test that exhausts memory takes the next
ones down with it, and a failure read off a whole-suite run can belong to
another test.

`.cargo/config.toml` caps linear memory at 256 MiB. Without it the 5.0.3
runaway walks to 4 GiB before it gives up.

## What is red on 5.0.3

| test | 5.0.3 | 5.1.0 |
|---|---|---|
| `aes_gcm_plaintext_size_does_not_run_away` | fails: allocation of 65,520 bytes fails after growing to the cap | passes |
| `extending_over_a_16_mib_gap_reuses_it` | fails: the 40 MiB request lands at `0x1550000` instead of the freed `0x130160` | passes |
| `freeing_above_a_16_mib_gap_does_not_grow_its_recorded_size` | fails: the gap's recorded size gains 33,554,432 bytes | passes |
| `aes_gcm_plaintext_size_does_not_run_away_when_extending` | passes | passes |
| `grow_and_extend_survives_history_sync_churn` | passes (1,629 pages) | passes (1,612 pages) |
| `extend_commits_less_than_claim_on_a_growing_buffer` | passes | passes |

The last two are guards rather than repros, and say so in their doc comments.
