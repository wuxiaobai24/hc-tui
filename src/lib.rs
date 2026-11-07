//! hc-tui: a terminal workbench for coding agents, built on `herdr-core`.
//!
//! 分层（详见 docs/plan-v1.1.md）：
//! - [`hc`]：与 core 的连接、帧编解码、RPC
//! - [`term`]：本地 VT 与到 Ratatui Buffer 的桥
//! - [`layout`]：BSP 分屏
//! - [`app`] / [`ui`]：状态与渲染
//! - [`tui`]：外层终端的进入与恢复

pub mod app;
pub mod chat;
pub mod clipboard;
pub mod config;
pub mod copy;
pub mod hc;
pub mod i18n;
pub mod layout;
pub mod model;
pub mod mods;
pub mod term;
pub mod theme;
pub mod tui;
pub mod ui;

pub mod shared;

pub mod store;

pub mod control;

pub mod update;
