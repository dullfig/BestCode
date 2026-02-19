//! WASM tool runtime — sandboxed user tools via WebAssembly components.
//!
//! Phase 5: The Immune System. Tools can't do harm because they literally
//! can't express harmful operations — missing WASI capabilities mean the
//! import doesn't exist, not that a policy check blocks it.
//!
//! Architecture:
//! - `runtime.rs` — WasmRuntime engine, component loading, metadata extraction
//! - `error.rs` — WasmError types
//! - `peer.rs` — WasmToolPeer: Handler + ToolPeer bridge (M2)
//! - `capabilities.rs` — WASI capability grants (M3)
//! - `definitions.rs` — WasmToolRegistry: auto-generated ToolDefinitions (M4)

pub mod capabilities;
pub mod definitions;
pub mod error;
pub mod peer;
pub mod runtime;
