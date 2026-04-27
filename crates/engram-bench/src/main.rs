//! `engram-bench` binary entry point.
//!
//! **Placeholder** established by `task:bench-impl-cargo-toml`. The full CLI
//! surface (subcommands `locomo`, `longmemeval`, `cost`, `test-preservation`,
//! `cognitive-regression`, `migration-integrity`, `release-gate`, `explain`,
//! `clean-fixtures`, with `--from-record`, `--override-gate`, `--rationale`
//! flags) is delivered by `task:bench-impl-main` per design §7.1.
//!
//! Until then, invoking `engram-bench` prints a stub message and exits with
//! code 2 (driver error per §7.1 exit-code table) so accidental CI invocations
//! fail loudly rather than silently passing.

fn main() -> std::process::ExitCode {
    eprintln!(
        "engram-bench: not yet implemented — see task:bench-impl-main \
         (v03-benchmarks build plan)."
    );
    std::process::ExitCode::from(2)
}
