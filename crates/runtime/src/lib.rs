//! Thread ownership and message transport for CounterSplat hosts.
//!
//! On native targets, `native::Worker` runs one subsystem on a named long-lived thread. Its state is constructed,
//! used, and dropped on that thread, so the state itself does not need to implement `Send`. Commands and replies
//! cross a bounded queue; callers get explicit backpressure and may poll instead of blocking a window/event loop.
//!
//! `native::Supervisor` groups independently typed workers and requests shutdown on all of them before joining.
//! It is a host-facing orchestration primitive: a host must register real asset, render, physics, simulation, and
//! scripting handlers. This crate does not implicitly start subsystems or move their state between threads.
//!
//! Native workers are unavailable on WebAssembly targets. The Rust `wasm32-unknown-unknown` standard library cannot
//! start OS threads; a browser host must create Web Workers and use message passing. Shared-memory WASM requires
//! extra browser and toolchain support. [`backend`] exposes that distinction rather than silently running work on
//! one thread. Other WebAssembly hosts must provide their own threading backend.

pub mod latest;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;

/// Which host must provide subsystem execution for the current target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadBackend {
    /// Native named threads can be started through the `native` module.
    Native,
    /// A WebAssembly host must supply its own implementation; browsers can use Web Workers.
    WebAssemblyHostRequired,
}

/// Returns the threading backend available to this build.
pub const fn backend() -> ThreadBackend {
    if cfg!(target_arch = "wasm32") {
        ThreadBackend::WebAssemblyHostRequired
    } else {
        ThreadBackend::Native
    }
}
