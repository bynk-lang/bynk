//! Project-failure rendering for the driver.
//!
//! #521: the flattening layer (ADR 0100) is shared with `bynk` in
//! [`bynk_driver`]; this module survives as re-exports under the driver's
//! historical `render_*` names.

pub use bynk_driver::print_project_failure as render_project_failure;
#[allow(unused_imports)]
pub use bynk_driver::print_project_failure_short as render_project_failure_short;
pub use bynk_driver::print_project_warnings;
