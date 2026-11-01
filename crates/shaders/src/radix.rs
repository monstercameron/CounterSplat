//! The GPU sort kernels, exposed as source rather than as sibling-crate file paths.

/// Four-bit radix histogram kernel.
pub const HISTOGRAM: &str = include_str!("../shaders/radix-sort/compute-radix-sort-4bit.wgsl");
/// Radix reorder kernel.
pub const REORDER: &str = include_str!("../shaders/radix-sort/compute-radix-sort-reorder.wgsl");
/// Prefix-sum kernel used by the radix sorter.
pub const PREFIX_SUM: &str = include_str!("../shaders/radix-sort/compute-prefix-sum.wgsl");
