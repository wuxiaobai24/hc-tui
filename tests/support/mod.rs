//! 对拍用例共用的模块等待与区域取样。
//!
//! 收帧条件取宿主记账的就绪状态（`props` 修订号与模块落笔修订号一致），
//! 或者取「派发之后内容真的变了」；不用连续两帧相同这类停机时间猜测。

#![allow(dead_code)]

use std::time::Duration;

use hc_tui::app::App;
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

/// 一帧里某块矩形的文本：逐行取字符。
pub fn region_text(buffer: &Buffer, area: Rect) -> String {
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

/// 画帧并泵模块通道，直到一帧同时满足：帧开始时每个模块已按交付的 props 画过
/// （就绪前这一帧会把内置帧盖回去，props 也可能就在这一帧里被推新），帧结束时
/// 仍就绪（这一帧没有推到新 props），而且这一帧真的画到了每个模块（帧预算耗尽
/// 时后面的表面会被跳过）。返回时终端上那一帧是模块自己的像素。
pub fn pump_until_ready(app: &mut App, terminal: &mut Terminal<TestBackend>, modules: &[&str]) {
    for _ in 0..1200 {
        let ready = modules.iter().all(|module| app.mod_ready(module));
        let before: Vec<u64> = modules
            .iter()
            .map(|module| app.mod_frames_drawn(module))
            .collect();
        terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
        app.pump_mods();
        let drawn = modules
            .iter()
            .zip(&before)
            .all(|(module, before)| app.mod_frames_drawn(module) > *before);
        let still_ready = modules.iter().all(|module| app.mod_ready(module));
        if ready && still_ready && drawn {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let revisions: Vec<String> = modules
        .iter()
        .map(|module| format!("{module}: {:?}", app.mod_paint_revisions(module)))
        .collect();
    panic!(
        "modules never painted with the delivered props: {}",
        revisions.join(", ")
    );
}

/// 投递输入之后画到模块落笔，最多 `budget` 帧：`before` 是投递前那块矩形的
/// 文本，画出别的内容才算这一笔落地（表面关掉也算）。超时返回 `false`，调用
/// 方可以重投；真坏了照样能判定。
pub fn pump_until_repainted_within(
    app: &mut App,
    terminal: &mut Terminal<TestBackend>,
    area: Rect,
    before: &str,
    budget: usize,
) -> bool {
    for _ in 0..budget {
        terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
        app.pump_mods();
        if region_text(terminal.backend().buffer(), area) != before {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

/// 投递输入之后画到模块落笔：`before` 是投递前那块矩形的文本，画出别的内容
/// 才算这一笔落地。表面关掉（矩形内容整体变了）同样算落地。
pub fn pump_until_repainted(
    app: &mut App,
    terminal: &mut Terminal<TestBackend>,
    area: Rect,
    before: &str,
) {
    if !pump_until_repainted_within(app, terminal, area, before, 1200) {
        panic!(
            "module never repainted after the input; frame now:\n{}",
            region_text(terminal.backend().buffer(), area)
        );
    }
}
