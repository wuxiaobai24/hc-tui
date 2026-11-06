//! 界面文案：代码里写英文原文，中文界面下由表格换成中文。
//!
//! 每个表格模块提供一个 `lookup(&'static str) -> Option<&'static str>`，
//! 按源码里的英文原文索引。表格里没有的原文原样返回，所以漏翻只影响文案，
//! 不影响运行。带占位符的文案统一用 `{}`，用 [`t1`] / [`t2`] / [`t3`] 依次填入。

mod actions;
mod app;
mod chat;
mod cli;
mod core;
mod ui;

use std::sync::atomic::{AtomicBool, Ordering};

static ZH: AtomicBool = AtomicBool::new(false);

/// 启动时按 `ui.lang` 设一次；`Config::load` 负责调用。
pub fn set_zh(on: bool) {
    ZH.store(on, Ordering::Relaxed);
}

/// 当前是否用中文界面。
pub fn zh() -> bool {
    ZH.load(Ordering::Relaxed)
}

/// 取当前语言的文案。
pub fn t(en: &'static str) -> &'static str {
    if !zh() {
        return en;
    }
    let tables: [fn(&str) -> Option<&'static str>; 6] = [
        actions::lookup,
        app::lookup,
        chat::lookup,
        cli::lookup,
        core::lookup,
        ui::lookup,
    ];
    for table in tables {
        if let Some(text) = table(en) {
            return text;
        }
    }
    en
}

/// 取运行期才拿到的文案（例如 core 返回的 `how` 取值）；没有表项时返回原文。
pub fn tv(text: &str) -> &str {
    if !zh() {
        return text;
    }
    let tables: [fn(&str) -> Option<&'static str>; 6] = [
        actions::lookup,
        app::lookup,
        chat::lookup,
        cli::lookup,
        core::lookup,
        ui::lookup,
    ];
    for table in tables {
        if let Some(zh) = table(text) {
            return zh;
        }
    }
    text
}

/// 取文案并填第一个 `{}`。
pub fn t1(en: &'static str, a: impl std::fmt::Display) -> String {
    t(en).replacen("{}", &a.to_string(), 1)
}

/// 取文案并依次填两个 `{}`。
pub fn t2(en: &'static str, a: impl std::fmt::Display, b: impl std::fmt::Display) -> String {
    t1(en, a).replacen("{}", &b.to_string(), 1)
}

/// 取文案并依次填三个 `{}`。
pub fn t3(
    en: &'static str,
    a: impl std::fmt::Display,
    b: impl std::fmt::Display,
    c: impl std::fmt::Display,
) -> String {
    t2(en, a, b).replacen("{}", &c.to_string(), 1)
}
