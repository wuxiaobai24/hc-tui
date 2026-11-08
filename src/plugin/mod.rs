//! 插件：清单、注册表、动作执行、终端入口与日志。
//!
//! 插件是独立进程的工作流包，提供一次性动作与真实终端入口。清单与注册表见
//! [`manifest`]、[`registry`]，动作执行见 [`runner`]，终端入口的规划与
//! 代际引用见 [`terminal`]、[`lease`]，常驻服务见 [`service`]。

pub mod lease;
pub mod logs;
pub mod manifest;
pub mod registry;
pub mod runner;
pub mod service;
pub mod terminal;
