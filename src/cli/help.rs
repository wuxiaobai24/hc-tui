//! `--help` 里的输出契约段：用法行之后的四行固定格式。
//!
//! 数据形状：`用法行` + `输出:` 段，人类形态 / `--json` / 退出码 / 常见错误各一行。
//! 模块边界：格式只在这里，条目由各命令给出；文案走 [`hc_tui::i18n::t`]。
//! 不变量：常见错误一栏只能列 [`ErrorCode`] 里的码。

use hc_tui::error::ErrorCode;
use hc_tui::i18n::t;

/// 输出契约段（不含用法行），以换行结尾。
pub fn block(human: &str, json: &str, codes: &[ErrorCode]) -> String {
    let mut names = Vec::with_capacity(codes.len());
    for code in codes {
        debug_assert!(
            ErrorCode::ALL.contains(code),
            "{} 不在错误码词表里",
            code.as_str()
        );
        names.push(code.as_str());
    }
    format!(
        "{}\n  {}: {}\n  --json:   {}\n  {}:   {}\n{}:  {}\n",
        t("output:"),
        t("human form"),
        human,
        json,
        t("exit codes"),
        t("0 ok / 1 failure / 2 usage / 3 core unavailable"),
        t("common errors"),
        names.join(", ")
    )
}

/// 用法行 + 输出契约段。
pub fn contract(usage: &str, human: &str, json: &str, codes: &[ErrorCode]) -> String {
    format!("{usage}\n{}", block(human, json, codes))
}
