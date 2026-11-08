//! `[[services]]` 的物化与回收：声明 → core 的 keep 终端 → 布局文档里的台账。
//!
//! 数据形状 = [`crate::plugin::registry::ServiceInfo`] + [`SavedService`]；台账
//! 写进 `hctui-layout-v1` 的 `services` 段，因此与其它布局改动共用同一条写入
//! 路径（[`App::flush_layout`] 的 CAS + 三方合并），不存在第二份写入。
//! 模块边界：清单与注册表给声明，[`crate::plugin::service`] 做纯规划，这里只
//! 做「问 core 要终端清单 → 决定起停 → 记台账」。
//!
//! 不变量：同一个 `(package, service)` 至多一个活终端——先按 core 的终端名
//! 认领既有终端，`terminal.run` 撞名（`name_taken`）时改为认领；禁用与卸载后
//! desired 集合不再包含它，台账里的地址一律 `terminal.kill` 并删记录。

use super::*;
use crate::model::SavedService;
use crate::plugin::service::{self, PlanError};

/// 什么时候对账；`on = "install"` 的服务只在安装/更新后起一次。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Trigger {
    /// 安装、更新、链接之后。
    Install,
    /// 启用、停用、卸载之后。
    Enable,
    /// 窗口重开（只认领与回收，不重启 `install` 服务）。
    #[default]
    Open,
}

impl Trigger {
    /// 这次对账是否允许拉起这条服务。
    fn starts(self, on: &str) -> bool {
        match on {
            "install" => self == Trigger::Install,
            _ => true,
        }
    }
}

impl App {
    /// 对账一次：状态的唯一来源是 core 的终端清单，不是本地台账。
    ///
    /// 要 `all`：已结束的终端记录是「不自动重启」的依据。
    pub(crate) fn services_reconcile(&mut self, trigger: Trigger) {
        self.service_trigger = trigger;
        if self.inflight.contains_key("svc:list") {
            return;
        }
        self.call_rpc("svc:list", "terminal.list", json!({"all": true}));
    }

    /// `svc:` 标签的 RPC 回复。
    pub(crate) fn service_reply(
        &mut self,
        tag: &str,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) {
        if tag == "svc:list" {
            self.service_list(result);
            return;
        }
        if let Some(name) = tag.strip_prefix("svc:run:") {
            self.service_started(name, result);
            return;
        }
        if tag.starts_with("svc:kill:") {
            if let Err(error) = result {
                self.note = t1("service kill failed: {}", error.message);
            }
        }
    }

    /// 按 core 的终端清单对账：先回收，再认领，最后按时机拉起。
    fn service_list(&mut self, result: std::result::Result<Value, crate::hc::net::RpcFailure>) {
        let terminals = match result {
            Ok(value) => value
                .get("terminals")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            Err(error) => {
                self.note = t1("service list failed: {}", error.message);
                self.dirty = true;
                return;
            }
        };
        // core 的终端名与标签 → 地址；已结束的终端单独记，保留台账、不再拉起。
        let mut live: BTreeMap<String, String> = BTreeMap::new();
        let mut live_addresses: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        let mut finished_addresses: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for terminal in &terminals {
            let Some(address) = terminal.get("address").and_then(Value::as_str) else {
                continue;
            };
            if terminal.get("state").and_then(Value::as_str) == Some("finished") {
                finished_addresses.insert(address.to_string());
                continue;
            }
            live_addresses.insert(address.to_string());
            if let Some(name) = terminal.get("name").and_then(Value::as_str) {
                live.insert(name.to_string(), address.to_string());
            }
            if let Some(label) = terminal
                .pointer("/labels/hctui.service")
                .and_then(Value::as_str)
            {
                live.insert(label.to_string(), address.to_string());
            }
        }
        let desired = self.plugins.services();
        let trigger = self.service_trigger;
        let mut ledger: Vec<SavedService> = std::mem::take(&mut self.services);
        // 回收：声明没了、代际变了、地址已被 core 丢掉。
        let wanted: BTreeMap<String, u64> = desired
            .iter()
            .map(|info| (info.name.clone(), info.generation))
            .collect();
        let mut kept: Vec<SavedService> = Vec::new();
        let mut changed = false;
        for entry in ledger.drain(..) {
            let current = wanted.get(&entry.id).copied();
            let stale = match current {
                None => true,
                Some(generation) => generation != entry.generation,
            };
            let live = live_addresses.contains(&entry.terminal);
            if !stale && (live || finished_addresses.contains(&entry.terminal)) {
                // 活着的认领；已结束的保留台账，这正是「不自动重启」。
                kept.push(entry);
                continue;
            }
            // 声明没了、代际变了、或地址已被 core 丢掉：台账都要改。
            if live {
                self.kill_service(&entry.terminal);
            }
            changed = true;
        }
        ledger = kept;
        // 拉起：已有台账条目的服务不重复起；core 里已有同名字终端的只认领。
        for info in &desired {
            if ledger.iter().any(|entry| entry.id == info.name) {
                continue;
            }
            let plan = match service::plan(info, &self.config.plugins_state_dir()) {
                Ok(mut plan) => {
                    // 与动作、终端入口同一套注入变量，包自己的 env 叠加在上面。
                    let mut env = self.plugin_env_base(&info.package, &info.root);
                    env.insert("HCTUI_PLUGIN_SERVICE".into(), info.service.clone());
                    env.extend(plan.env);
                    plan.env = env;
                    plan
                }
                Err(error) => {
                    self.record_service_issue(&info.name, &error);
                    continue;
                }
            };
            if let Some(address) = live
                .get(&plan.terminal_name)
                .or_else(|| live.get(&plan.name))
                .cloned()
            {
                ledger.push(SavedService {
                    id: info.name.clone(),
                    package: info.package.clone(),
                    service: info.service.clone(),
                    generation: info.generation,
                    terminal: address,
                    started_ms: service::now_ms(),
                });
                changed = true;
                continue;
            }
            if !trigger.starts(&info.on) {
                continue;
            }
            self.service_pending
                .insert(info.name.clone(), (plan.clone(), info.generation));
            self.call_rpc(
                &format!("svc:run:{}", info.name),
                "terminal.run",
                service::run_params(&plan),
            );
        }
        ledger.sort_by(|a, b| a.id.cmp(&b.id));
        self.services = ledger;
        if changed {
            self.mark_layout();
            // 立刻提交，别让排队里的旧 doc.changed 抢在写入前把台账合并成冲突。
            self.flush_layout();
        }
    }

    /// `terminal.run` 的回复：地址进台账；撞名说明别人先起了，改去认领。
    fn service_started(
        &mut self,
        name: &str,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) {
        let Some((plan, generation)) = self.service_pending.remove(name) else {
            self.services_reconcile(Trigger::Open);
            return;
        };
        match result {
            Ok(value) => {
                let Some(address) = value.get("address").and_then(Value::as_str) else {
                    self.record_service_issue(
                        name,
                        &PlanError {
                            kind: service::UNAVAILABLE,
                            message: t("terminal.run returned no address").to_string(),
                        },
                    );
                    return;
                };
                if let Some(entry) = self.services.iter_mut().find(|entry| entry.id == name) {
                    entry.generation = generation;
                    entry.terminal = address.to_string();
                    entry.started_ms = service::now_ms();
                } else {
                    self.services.push(SavedService {
                        id: plan.name.clone(),
                        package: plan.package.clone(),
                        service: plan.service.clone(),
                        generation,
                        terminal: address.to_string(),
                        started_ms: service::now_ms(),
                    });
                }
                self.services.sort_by(|a, b| a.id.cmp(&b.id));
                self.mark_layout();
                self.flush_layout();
                self.note = t2("service {} started at {}", name, address);
                self.dirty = true;
            }
            Err(error) if error.kind == "name_taken" => {
                // 另一个写入者已经起了同一条服务：重新取清单认领它的地址。
                self.services_reconcile(self.service_trigger);
            }
            Err(error) => {
                let missing = error.kind == "spawn_failed"
                    || error.message.contains("No such file or directory");
                self.record_service_issue(
                    name,
                    &PlanError {
                        kind: if missing {
                            service::MISSING_PROGRAM
                        } else {
                            service::UNAVAILABLE
                        },
                        message: error.message,
                    },
                );
            }
        }
    }

    /// 停掉一条服务终端；地址已消失时 core 的报错不影响台账。
    fn kill_service(&mut self, address: &str) {
        self.call_rpc(
            &format!("svc:kill:{address}"),
            "terminal.kill",
            json!({ "terminal": address }),
        );
        self.note = t1("service stopped at {}", address);
        self.dirty = true;
    }

    /// 记一条服务问题；`packages list` 用同一份判定，不静默。
    fn record_service_issue(&mut self, name: &str, error: &PlanError) {
        let message = format!("service {name}: {}: {}", error.kind, error.message);
        if !self.service_issues.contains(&message) {
            self.service_issues.push(message.clone());
        }
        self.note = message;
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        App::with_network(
            Config {
                restore: false,
                state_path: "/nonexistent/hctui-state".into(),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }

    #[test]
    fn install_only_services_start_only_after_an_install() {
        assert!(Trigger::Install.starts("install"));
        assert!(!Trigger::Enable.starts("install"));
        assert!(!Trigger::Open.starts("install"));
        assert!(Trigger::Open.starts("enable"));
        assert!(Trigger::Enable.starts("enable"));
    }

    #[test]
    fn reconciliation_asks_core_for_the_terminal_list_first() {
        let mut app = app();
        app.services_reconcile(Trigger::Open);
        assert_eq!(app.service_trigger, Trigger::Open);
        assert_eq!(app.inflight["svc:list"].0, "terminal.list");
        // 同一轮里不重复发。
        app.services_reconcile(Trigger::Open);
        assert_eq!(app.inflight.len(), 1);
    }

    #[test]
    fn a_terminal_missing_from_core_drops_its_ledger_entry() {
        let mut app = app();
        app.services.push(SavedService {
            id: "dev.example.svc.watch".into(),
            package: "dev.example.svc".into(),
            service: "watch".into(),
            generation: 1,
            terminal: "t9".into(),
            started_ms: 1,
        });
        // core 的清单里没有 t9：台账清掉，不发第二个 run。
        app.service_reply(
            "svc:list",
            Ok(json!({"terminals": [], "hidden_finished": 0})),
        );
        assert!(app.services.is_empty(), "{:?}", app.services);
        assert!(!app
            .inflight
            .values()
            .any(|(method, _)| method == "terminal.run"));
    }

    #[test]
    fn a_live_terminal_outside_the_declared_set_is_killed() {
        let mut app = app();
        app.services.push(SavedService {
            id: "dev.example.svc.watch".into(),
            package: "dev.example.svc".into(),
            service: "watch".into(),
            generation: 1,
            terminal: "t4".into(),
            started_ms: 1,
        });
        // 没有包声明这条服务：台账清掉并停终端。
        app.service_reply(
            "svc:list",
            Ok(json!({"terminals": [{"address": "t4", "labels": {}}], "hidden_finished": 0})),
        );
        assert!(app.services.is_empty(), "{:?}", app.services);
        assert!(app
            .inflight
            .values()
            .any(|(method, params)| method == "terminal.kill" && params["terminal"] == "t4"));
    }
}
