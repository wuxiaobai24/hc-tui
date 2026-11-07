//! 插件：清单、注册表、动作执行与日志。
//!
//! 插件是独立进程的工作流包，提供一次性动作；终端入口在 M7 接入。清单与
//! 注册表见 [`manifest`]、[`registry`]，动作执行见 [`runner`]。

pub mod logs;
pub mod manifest;
pub mod registry;
pub mod runner;
