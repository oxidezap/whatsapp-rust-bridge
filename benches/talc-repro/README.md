# The talc bugs, as tests

This bridge used talc as its `#[global_allocator]` and dropped it in June 2026,
because talc 5.0.3 had two bugs that this workload reached in production. Both
were fixed in 5.0.4 and both are pinned here, so the next person who wants to
reopen the question starts from a red/green run instead of a changelog entry.

It is a standalone crate on purpose: showing a fix means running the same test
bodies against two versions of talc, and the bridge's own manifest pins one.

Every command below runs from this directory, so that cargo picks up this
manifest and the `.cargo/config.toml` beside it rather than the bridge's:

```sh
cd benches/talc-repro

# green, the version the bridge would take
cargo update -p talc --precise 5.1.0
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  WASM_BINDGEN_TEST_ONLY_NODE=1 cargo test --target wasm32-unknown-unknown

# red, the version that broke
cargo update -p talc --precise 5.0.3
```

Run each test on its own. The tests share one wasm instance and one linear
memory, so a test that exhausts memory takes the next ones down with it, and a
failure read off a whole-suite run can belong to another test:

```sh
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  WASM_BINDGEN_TEST_ONLY_NODE=1 \
  cargo test --target wasm32-unknown-unknown --lib \
  -- --exact repro::tests::aes_gcm_plaintext_size_does_not_run_away
```

The runner variables are not optional and `.cargo/config.toml` does not set
them: without them cargo tries to execute the wasm artifact directly.

`--exact` matches the **registered** name, which carries the module path. A bare
`aes_gcm_plaintext_size_does_not_run_away` matches nothing and the run still
exits 0, which on the broken version reads exactly like a pass. The names in the
table below are given in full for that reason.

`.cargo/config.toml` caps linear memory at 256 MiB. Without it the 5.0.3
runaway walks to 4 GiB before it gives up.

## What is red on 5.0.3

| test | 5.0.3 | 5.1.0 |
|---|---|---|
| `repro::tests::aes_gcm_plaintext_size_does_not_run_away` | fails: allocation of 65,520 bytes fails after growing to the cap | passes |
| `repro::tests::extending_over_a_16_mib_gap_reuses_it` | fails: the 40 MiB request lands at `0x1550000` instead of the freed `0x130160` | passes |
| `repro::tests::freeing_above_a_16_mib_gap_does_not_grow_its_recorded_size` | fails: the gap's recorded size gains 33,554,432 bytes | passes |
| `repro::tests::aes_gcm_plaintext_size_does_not_run_away_when_extending` | passes | passes |
| `stress::tests::grow_and_extend_survives_history_sync_churn` | passes (1,617 pages) | passes (1,605 pages) |
| `stress::tests::extend_commits_less_than_claim_on_a_growing_buffer` | passes | passes |
| `upstream::tests::a_doubling_buffer_is_copied_about_once_in_full` | passes | passes |
| `upstream::tests::memory_grow_hands_back_zeroed_pages` | passes | passes |

The last four are not repros and say so in their doc comments. The two stress
tests are guards for the next allocator change; the two `upstream` ones are the
runnable half of the "what both of them miss" section of
`docs/wasm-allocator-talc-5-1-0.md`, which is about gaps shared by dlmalloc and
talc rather than about anything talc got wrong.
