//! `hc-tui completion <shell>`：打印 zsh / bash / fish 的补全脚本。
//!
//! 脚本是静态文本；包 id 与 `do` 的命令名在补全时现取（`packages list --json`
//! 与 `commands`），取不到就静默留空，不阻塞补全。

use anyhow::Result;

/// 支持的 shell，按 `hc-tui completion` 无参数时的打印顺序。
const SHELLS: [&str; 3] = ["zsh", "bash", "fish"];

/// `hc-tui completion [<shell>]`：带 shell 打印脚本，不带参数列出支持的 shell。
pub fn run(args: &[String]) -> Result<()> {
    let Some(shell) = args.first().map(String::as_str) else {
        println!("usage: hc-tui completion <shell>");
        println!("shells: {}", SHELLS.join(" "));
        return Ok(());
    };
    match shell {
        "zsh" => print!("{ZSH}"),
        "bash" => print!("{BASH}"),
        "fish" => print!("{FISH}"),
        other => anyhow::bail!("usage: hc-tui completion zsh|bash|fish (unknown: {other})"),
    }
    Ok(())
}

/// zsh：`source <(hc-tui completion zsh)`，或存成 `$fpath` 里的 `_hc-tui`。
const ZSH: &str = r##"#compdef hc-tui
# hc-tui completion zsh —— source <(hc-tui completion zsh)，或存成 $fpath 里的 _hc-tui。

_hc-tui-package-ids() {
  local -a ids
  ids=(${(f)"$(hc-tui packages list --json 2>/dev/null | sed -n 's/^    "id": "\([^"]*\)",\{0,1\}$/\1/p')"})
  (( ${#ids} )) || return 1
  _describe -t packages package ids
}

_hc-tui-commands() {
  local -a names
  names=(${(f)"$(hc-tui commands 2>/dev/null | sed -n 's/^    "name": "\([^"]*\)",\{0,1\}$/\1/p')"})
  (( ${#names} )) || return 1
  _describe -t commands command names
}

_hc-tui-packages() {
  local -a subcommands
  subcommands=(
    'list:list installed packages'
    'install:install from a directory, .nshpkg, git url or .tar.gz'
    'update:update an installed package'
    'link:link a local directory'
    'unlink:unlink a linked package'
    'uninstall:uninstall a package'
    'enable:enable a package'
    'disable:disable a package'
    'gc:collect generations nothing references'
    'log:print a package log'
    'info:print package info'
  )
  _arguments -C \
    '1:subcommand:->subcommand' \
    '*::argument:->argument'
  case $state in
    subcommand) _describe -t subcommands 'packages subcommand' subcommands ;;
    argument)
      case $words[1] in
        install|update|link) _files -/ ;;
        info|enable|disable|uninstall|unlink) _hc-tui-package-ids ;;
      esac
      ;;
  esac
}

_hc-tui-plugins() {
  local -a subcommands
  subcommands=(
    'action:list, invoke, cancel or inspect plugin actions'
    'pane:open, focus or close plugin terminals'
    'list:list installed packages'
    'install:install from a directory, .nshpkg, git url or .tar.gz'
    'update:update an installed package'
    'link:link a local directory'
    'unlink:unlink a linked package'
    'uninstall:uninstall a package'
    'enable:enable a package'
    'disable:disable a package'
    'gc:collect generations nothing references'
    'log:print a package log'
    'info:print package info'
  )
  _arguments -C \
    '1:subcommand:->subcommand' \
    '*::argument:->argument'
  case $state in
    subcommand) _describe -t subcommands 'plugins subcommand' subcommands ;;
    argument)
      case $words[1] in
        action) _values 'action subcommand' list invoke status cancel log ;;
        pane) _values 'pane subcommand' open status focus close ;;
        install|update|link) _files -/ ;;
        info|enable|disable|uninstall|unlink) _hc-tui-package-ids ;;
      esac
      ;;
  esac
}

_hc-tui() {
  local -a commands
  commands=(
    'packages:manage installed packages'
    'plugins:run plugin actions and terminals'
    'do:run a TUI command'
    'events:stream events as NDJSON'
    'commands:list TUI commands'
    'mods:manage modules'
    'update:update hc-tui'
    'probe:probe the core'
    'session-hook:install, remove or inspect the chat hook'
    'completion:print a shell completion script'
    'config:check the config file, reset [keys], dump or init'
  )
  _arguments -C \
    '1:command:->command' \
    '*::argument:->argument'
  case $state in
    command) _describe -t commands 'hc-tui command' commands ;;
    argument)
      case $words[1] in
        packages) _hc-tui-packages ;;
        plugins) _hc-tui-plugins ;;
        do) _hc-tui-commands ;;
        completion) _values 'shell' zsh bash fish ;;
        config) _values 'config subcommand' check reset-keys dump init ;;
      esac
      ;;
  esac
}

if [[ $funcstack[1] == _hc-tui ]]; then
  _hc-tui "$@"
else
  if (( $+functions[compdef] )); then
    compdef _hc-tui hc-tui
  fi
fi
"##;

/// bash：`source <(hc-tui completion bash)`。
const BASH: &str = r##"# hc-tui completion bash —— source <(hc-tui completion bash)

__hc_tui_package_ids() {
  hc-tui packages list --json 2>/dev/null | sed -n 's/^    "id": "\([^"]*\)",\{0,1\}$/\1/p'
}

__hc_tui_command_names() {
  hc-tui commands 2>/dev/null | sed -n 's/^    "name": "\([^"]*\)",\{0,1\}$/\1/p'
}

_hc_tui() {
  local cur sub subsub i index
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  sub=""
  for ((i = 1; i < COMP_CWORD; i++)); do
    case "${COMP_WORDS[i]}" in
      --config|--session|--attach|--from|--version)
        i=$((i + 1))
        ;;
      -*)
        ;;
      *)
        sub="${COMP_WORDS[i]}"
        break
        ;;
    esac
  done
  if [[ -z "$sub" ]]; then
    COMPREPLY=($(compgen -W "packages plugins do events commands mods update probe session-hook completion config" -- "$cur"))
    return 0
  fi
  index=$((COMP_CWORD - i))
  case "$sub" in
    packages)
      if [[ $index -le 1 ]]; then
        COMPREPLY=($(compgen -W "list install update link unlink uninstall enable disable gc log info" -- "$cur"))
        return 0
      fi
      subsub="${COMP_WORDS[i + 1]}"
      case "$subsub" in
        install|update|link) COMPREPLY=($(compgen -d -- "$cur")) ;;
        info|enable|disable|uninstall|unlink)
          COMPREPLY=($(compgen -W "$(__hc_tui_package_ids)" -- "$cur"))
          ;;
      esac
      ;;
    plugins)
      if [[ $index -le 1 ]]; then
        COMPREPLY=($(compgen -W "action pane list install update link unlink uninstall enable disable gc log info" -- "$cur"))
        return 0
      fi
      subsub="${COMP_WORDS[i + 1]}"
      if [[ $index -eq 2 ]]; then
        case "$subsub" in
          action) COMPREPLY=($(compgen -W "list invoke status cancel log" -- "$cur")) ;;
          pane) COMPREPLY=($(compgen -W "open status focus close" -- "$cur")) ;;
          install|update|link) COMPREPLY=($(compgen -d -- "$cur")) ;;
          info|enable|disable|uninstall|unlink)
            COMPREPLY=($(compgen -W "$(__hc_tui_package_ids)" -- "$cur"))
            ;;
        esac
      fi
      ;;
    do)
      if [[ $index -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$(__hc_tui_command_names)" -- "$cur"))
      fi
      ;;
    completion)
      if [[ $index -eq 1 ]]; then
        COMPREPLY=($(compgen -W "zsh bash fish" -- "$cur"))
      fi
      ;;
    config)
      if [[ $index -eq 1 ]]; then
        COMPREPLY=($(compgen -W "check reset-keys dump init" -- "$cur"))
      fi
      ;;
  esac
  return 0
}

complete -F _hc_tui hc-tui
"##;

/// fish：`hc-tui completion fish | source`。
const FISH: &str = r##"# hc-tui completion fish —— hc-tui completion fish | source

function __hc_tui_package_ids
    hc-tui packages list --json 2>/dev/null | sed -n 's/^    "id": "\([^"]*\)",\{0,1\}$/\1/p'
end

function __hc_tui_command_names
    hc-tui commands 2>/dev/null | sed -n 's/^    "name": "\([^"]*\)",\{0,1\}$/\1/p'
end

function __hc_tui_packages_subcommand
    __fish_seen_subcommand_from packages; and not __fish_seen_subcommand_from list install update link unlink uninstall enable disable gc log info
end

function __hc_tui_plugins_subcommand
    __fish_seen_subcommand_from plugins; and not __fish_seen_subcommand_from action pane list install update link unlink uninstall enable disable gc log info
end

complete -c hc-tui -f
complete -c hc-tui -n '__fish_use_subcommand' -a packages -d 'manage installed packages'
complete -c hc-tui -n '__fish_use_subcommand' -a plugins -d 'run plugin actions and terminals'
complete -c hc-tui -n '__fish_use_subcommand' -a do -d 'run a TUI command'
complete -c hc-tui -n '__fish_use_subcommand' -a events -d 'stream events as NDJSON'
complete -c hc-tui -n '__fish_use_subcommand' -a commands -d 'list TUI commands'
complete -c hc-tui -n '__fish_use_subcommand' -a mods -d 'manage modules'
complete -c hc-tui -n '__fish_use_subcommand' -a update -d 'update hc-tui'
complete -c hc-tui -n '__fish_use_subcommand' -a probe -d 'probe the core'
complete -c hc-tui -n '__fish_use_subcommand' -a session-hook -d 'install or remove the chat hook'
complete -c hc-tui -n '__fish_use_subcommand' -a completion -d 'print a shell completion script'
complete -c hc-tui -n '__fish_use_subcommand' -a config -d 'check the config file, reset [keys], dump or init'

complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a list -d 'list installed packages'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a install -d 'install from a directory, .nshpkg, git url or .tar.gz'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a update -d 'update an installed package'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a link -d 'link a local directory'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a unlink -d 'unlink a linked package'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a uninstall -d 'uninstall a package'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a enable -d 'enable a package'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a disable -d 'disable a package'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a gc -d 'collect generations nothing references'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a log -d 'print a package log'
complete -c hc-tui -f -n '__hc_tui_packages_subcommand' -a info -d 'print package info'

complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a action -d 'list, invoke, cancel or inspect plugin actions'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a pane -d 'open, focus or close plugin terminals'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a list -d 'list installed packages'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a install -d 'install from a directory, .nshpkg, git url or .tar.gz'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a update -d 'update an installed package'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a link -d 'link a local directory'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a unlink -d 'unlink a linked package'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a uninstall -d 'uninstall a package'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a enable -d 'enable a package'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a disable -d 'disable a package'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a gc -d 'collect generations nothing references'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a log -d 'print a package log'
complete -c hc-tui -f -n '__hc_tui_plugins_subcommand' -a info -d 'print package info'

complete -c hc-tui -f -n '__fish_seen_subcommand_from action' -a 'list invoke status cancel log'
complete -c hc-tui -f -n '__fish_seen_subcommand_from pane' -a 'open status focus close'

complete -c hc-tui -f -n '__fish_seen_subcommand_from install link update' -a '(__fish_complete_directories)'
complete -c hc-tui -f -n '__fish_seen_subcommand_from info enable disable uninstall unlink' -a '(__hc_tui_package_ids)'
complete -c hc-tui -f -n '__fish_seen_subcommand_from do' -a '(__hc_tui_command_names)'
complete -c hc-tui -f -n '__fish_seen_subcommand_from completion' -a 'zsh bash fish'
complete -c hc-tui -f -n '__fish_seen_subcommand_from config' -a 'check reset-keys dump init'
"##;
