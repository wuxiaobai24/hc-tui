fn main() {
    // `doctor` 的 `[VERSION]` 要真 target triple：build script 里 `TARGET` 不保证有，
    // 从 rustc 的 `host:` 行取，缺省交给编译期 env!("HC_TUI_TARGET") 兜底。
    let target =
        std::process::Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
            .arg("-vV")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
            })
            .unwrap_or_else(|| std::env::var("TARGET").unwrap_or_else(|_| "unknown".into()));
    println!("cargo:rustc-env=HC_TUI_TARGET={target}");
}
