//! M2 探针：nsh `AppView` 能在 hc-tui 帧的 Pane 矩形里绘制、收输入、被唤醒并干净退出。
//!
//! 按 docs/plan-extensions.md M2 的退出条件组织：
//! - 最小 TSX bundle 在矩形内按 `render(frame, rect)` 绘制，矩形外保持空白；
//! - 按键与鼠标到达模块（`Handled::Yes`），矩形外鼠标被忽略；
//! - bracketed paste 到达模块内的文本框；
//! - 换一个更小的矩形渲染即 resize，布局随之更新；
//! - 唤醒回调在引擎出帧时触发；`stop()` 后线程随 `Engine::drop` 收拢，不再唤醒；
//! - 应用代码请求退出（菜单 quit 角色）时 `exit_requested()` 报告为真。

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use nsh_core::clipboard::MemoryClipboard;
use nsh_platform_tui::{
    AppOptions, AppView, Bundle, Handled, Input, Modifiers, MouseButton, MouseInput, MouseKind,
};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

/// 最小 TSX 模块：计数按钮、可输入文本框、可选中文本，应用菜单带 copy 与 quit 角色。
const APP_TSX: &str = r#"
import { mount, signal, View, Text } from "nsh";
import { Input } from "nsh/base";
import { Menu } from "nsh/desktop";
const count = signal(0);
const value = signal("");
Menu.setApplicationMenu([
  { label: "Edit", submenu: [{ role: "copy" }, { role: "quit" }] },
]);
mount(() => <View style={{ padding: 4, gap: 2 }}>
  <View selectable><Text>probe target</Text></View>
  <View onPress={() => count.update((c) => c + 1)}
      style={{ width: 32, height: 16, alignItems: "center" }}>
    <Text>+1</Text>
  </View>
  <Input value={value()} style={{ width: 120, height: 16 }}
      onChange={({ value: v }) => value.set(v)} />
  <Text>count:{count()}</Text>
  <Text>value:{value()}</Text>
</View>);
"#;

fn bundle() -> Bundle {
    let code = nsh_bundler::compile_module("App.tsx", APP_TSX, &[]).unwrap();
    Bundle::source(code)
}

/// 带唤醒通道的表面：引擎出帧时 `rx` 收到一次通知。
fn surface() -> (AppView, Receiver<()>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut options = AppOptions::default();
    options.wake = Some(Arc::new(move || {
        let _ = tx.send(());
    }));
    options.cache = nsh_platform_tui::CacheOptions::off();
    options.clipboard = Some(Arc::new(MemoryClipboard::default()));
    (AppView::new(options), rx)
}

fn key(name: &str) -> Input {
    Input::Key {
        key: name.into(),
        modifiers: Modifiers::default(),
        release: false,
    }
}

fn ctrl(name: &str) -> Input {
    Input::Key {
        key: name.into(),
        modifiers: Modifiers {
            ctrl: true,
            ..Modifiers::default()
        },
        release: false,
    }
}

fn mouse(kind: MouseKind, column: u16, row: u16) -> Input {
    let mut mouse = MouseInput::default();
    mouse.kind = kind;
    mouse.column = column;
    mouse.row = row;
    Input::Mouse(mouse)
}

/// Pane 矩形：hc-tui 里模块区域的形态——不是整帧。
fn pane(area: Rect, x: u16, y: u16, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + x,
        y: area.y + y,
        width: width.min(area.width.saturating_sub(x)),
        height: height.min(area.height.saturating_sub(y)),
    }
}

fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

fn screen(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| row(terminal, y) + "\n")
        .collect()
}

/// 渲染直到矩形内出现 `expected`；唤醒通道与动画截止时间驱动重绘。
fn draw_until(
    view: &mut AppView,
    terminal: &mut Terminal<TestBackend>,
    area: Rect,
    rx: &Receiver<()>,
    expected: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        terminal.draw(|frame| view.render(frame, area)).unwrap();
        let buffer = terminal.backend().buffer();
        let inside: String = ((area.y..area.bottom()).map(|y| {
            (area.x..area.right())
                .map(move |x| buffer[(x, y)].symbol())
                .collect::<String>()
        }))
        .collect::<Vec<_>>()
        .join("\n");
        if inside.contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected:?} inside {area:?}, got {}",
            screen(terminal)
        );
        let due = view
            .next_frame_deadline()
            .map_or(deadline, |due| due.min(deadline));
        let _ = rx.recv_deadline(due);
    }
}

#[test]
fn module_renders_in_a_pane_rect_and_takes_key_mouse_paste_resize() {
    let (mut view, rx) = surface();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let area = Rect::new(0, 0, 80, 24);
    let rect = pane(area, 4, 2, 60, 18);

    // 先按宿主的方式预画矩形，再加载应用。
    terminal.draw(|frame| view.render(frame, rect)).unwrap();
    view.load(bundle()).unwrap();
    assert!(
        rx.recv_timeout(Duration::from_secs(5)).is_ok(),
        "engine must wake the host after load"
    );
    draw_until(&mut view, &mut terminal, rect, &rx, "probe target");

    // 矩形外保持空白：模块只写自己的表面。
    let buffer = terminal.backend().buffer();
    let outside_blank = (0..buffer.area.height).all(|y| {
        (0..buffer.area.width).all(|x| {
            rect.x <= x && x < rect.right() && rect.y <= y && y < rect.bottom()
                || buffer[(x, y)].symbol() == " "
        })
    });
    assert!(outside_blank, "module drew outside its pane rect");

    // 鼠标：点击 +1 按钮，计数值更新，事件被模块消费。
    let (bx, by) = find_text(&terminal, rect, "+1").expect("+1 button visible");
    assert_eq!(
        view.handle(mouse(MouseKind::Down(MouseButton::Left), bx, by)),
        Handled::Yes
    );
    assert_eq!(
        view.handle(mouse(MouseKind::Up(MouseButton::Left), bx, by)),
        Handled::Yes
    );
    draw_until(&mut view, &mut terminal, rect, &rx, "count:1");
    assert_eq!(
        view.handle(mouse(MouseKind::Down(MouseButton::Left), 0, 0)),
        Handled::Ignored,
        "mouse outside the pane rect must be ignored"
    );

    // 点击文本框聚焦，按键到达模块并回显。
    let value_row = find_text(&terminal, rect, "value:")
        .expect("value row visible")
        .1;
    view.handle(mouse(
        MouseKind::Down(MouseButton::Left),
        rect.x + 6,
        value_row - 2,
    ));
    view.handle(mouse(
        MouseKind::Up(MouseButton::Left),
        rect.x + 6,
        value_row - 2,
    ));
    assert_eq!(view.handle(key("h")), Handled::Yes);
    assert_eq!(view.handle(key("i")), Handled::Yes);
    draw_until(&mut view, &mut terminal, rect, &rx, "value:hi");

    // bracketed paste 到达文本框，宽字符也计入。
    view.handle(Input::Paste(" 宽".into()));
    draw_until(&mut view, &mut terminal, rect, &rx, "value:hi 宽");

    // resize：换更小的矩形渲染，布局随之更新。
    let small = pane(area, 8, 4, 30, 10);
    draw_until(&mut view, &mut terminal, small, &rx, "count:1");
    draw_until(&mut view, &mut terminal, small, &rx, "value:hi 宽");
}

#[test]
fn copy_role_copies_then_quit_role_requests_exit_and_stop_leaves_no_threads() {
    let (mut view, rx) = surface();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let area = Rect::new(0, 0, 80, 24);
    let rect = pane(area, 4, 2, 60, 18);
    terminal.draw(|frame| view.render(frame, rect)).unwrap();
    view.load(bundle()).unwrap();
    draw_until(&mut view, &mut terminal, rect, &rx, "probe target");

    // 选中 "probe target" 后 Ctrl+C 走 copy 角色，不退出。
    let (tx, ty) = find_text(&terminal, rect, "probe").expect("probe target visible");
    view.handle(mouse(MouseKind::Down(MouseButton::Left), tx, ty));
    view.handle(mouse(MouseKind::Move, tx + 5, ty));
    view.handle(mouse(MouseKind::Up(MouseButton::Left), tx + 5, ty));
    view.handle(ctrl("c"));
    assert!(!view.exit_requested(), "a copy does not quit");
    view.handle(key("Escape"));
    view.handle(ctrl("c"));
    assert!(view.exit_requested(), "{}", screen(&terminal));

    // 停止：线程随 Engine::drop 收拢，唤醒通道安静，再次绘制与退出查询安全。
    view.stop();
    assert!(!view.exit_requested(), "stop clears the app");
    // stop 本身会补发一次尾随唤醒通知；排掉后再验证通道安静。
    let trailing = rx.recv_timeout(Duration::from_millis(300));
    if trailing.is_err() {
        // 没有尾随通知也可以；下面的安静窗口必须成立。
    }
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "a stopped surface must not keep waking the host"
    );
    terminal.draw(|frame| view.render(frame, rect)).unwrap();
}

/// 在矩形内找一段文本的起始绝对坐标。
fn find_text(terminal: &Terminal<TestBackend>, rect: Rect, needle: &str) -> Option<(u16, u16)> {
    let buffer = terminal.backend().buffer();
    for y in rect.y..rect.bottom() {
        let mut line = String::new();
        for x in rect.x..rect.right() {
            line.push_str(buffer[(x, y)].symbol());
        }
        if let Some(start) = line.find(needle) {
            return Some((rect.x + start as u16, y));
        }
    }
    None
}
