//! M1 升级前后帧快照对比辅助（临时文件，不进入版本库）。
//! 用 ratatui facade 路径，保证 0.29 与 0.30 都能编译。
//! 输出写到 target/snapshot-<w>x<h>.txt，由外部脚本 diff 对比。

use hc_tui::app::App;
use hc_tui::config::Config;
use hc_tui::ui;
use ratatui::{backend::TestBackend, Terminal};

fn dump(w: u16, h: u16, path: &str) {
    let mut app = App::new(Config {
        restore: false,
        state_path: std::path::PathBuf::from("/nonexistent/hctui-snapshot"),
        ..Config::default()
    })
    .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
    let buf = terminal.backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let cell = &buf[(x, y)];
            out.push_str(&format!(
                "({x},{y})|{}|{:?}|{:?}|{:?}\n",
                cell.symbol(),
                cell.fg,
                cell.bg,
                cell.modifier
            ));
        }
    }
    std::fs::write(std::path::Path::new(path), out).unwrap();
}

#[test]
fn dump_upgrade_snapshots() {
    dump(80, 24, "target/snapshot-80x24.txt");
    dump(120, 40, "target/snapshot-120x40.txt");
}
