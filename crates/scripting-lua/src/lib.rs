//! Lua scripting for CounterSplat.
//!
//! The VM belongs to the thread that creates it. All access to the ECS goes
//! through owned WorldOperation messages; the Lua side never holds a reference
//! to the world. See the crate README for the Lua API, thread integration,
//! conversion rules, and platform requirements.

#![forbid(unsafe_code)]

mod bridge;
mod runtime;
mod worker;

pub use bridge::{ChannelBridge, WorldBridge, WorldRequest};
pub use runtime::{LuaRuntime, ScriptOptions};
pub use worker::{ScriptReply, ScriptWorker};
