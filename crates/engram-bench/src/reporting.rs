//! Output reporting — summary table, per-gate drill-down, regression
//! alerting (design §10).
//!
//! - §10.1 Summary table — single-line gate status per driver.
//! - §10.2 Per-gate drill-down — for failed gates, prints the per-query
//!   contributions sorted by contribution to the failure.
//! - §10.3 Regression alerting — diff vs the previous committed
//!   reproducibility record.
//!
//! ## Status
//!
//! **Structural placeholder.** Public formatters (`render_summary_table()`,
//! `render_drilldown()`, `render_diff()`) are owned by
//! `task:bench-impl-reporting`.
