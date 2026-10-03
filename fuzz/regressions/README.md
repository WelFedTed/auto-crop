# Fuzz regressions

Inputs that made a fuzz target crash, hang or break an invariant (ROADMAP M1.70, M1.71): one directory
per target (`probe/`, `limits/`, `metadata/`, `editstate_json/`), one file per input, named
`<short-description>-<first 8 hex of sha256>`. `cargo test -p xtask --test fuzz_regressions` runs every
file through its target's entry function on all three OSes, without libFuzzer, so a fixed bug stays
fixed. See [docs/testing/fuzzing.md](../../docs/testing/fuzzing.md).

Add a file in the same commit as the fix: the test must pass on `main`. A crasher that is not fixed
yet is kept in the issue that tracks it.
