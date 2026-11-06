//! Branch probes run off the UI thread; each directory has at most one probe.
use super::*;
use std::collections::HashSet;
use std::process::{Command as ProcessCommand, Stdio};

pub(super) struct BranchProbes {
    pending: HashSet<String>,
    sender: std::sync::mpsc::Sender<(String, String)>,
    receiver: Receiver<(String, String)>,
}

impl Default for BranchProbes {
    fn default() -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            pending: HashSet::new(),
            sender,
            receiver,
        }
    }
}

impl BranchProbes {
    pub(super) fn refresh(&mut self, cwd: &str) {
        if self.pending.len() >= 4 || !self.pending.insert(cwd.to_owned()) {
            return;
        }
        let cwd = cwd.to_owned();
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let mut command = ProcessCommand::new("git");
            command.args(["-C", &cwd, "rev-parse", "--abbrev-ref", "HEAD"]);
            let branch = probe(&mut command, Duration::from_secs(2)).unwrap_or_default();
            let _ = sender.send((cwd, branch));
        });
    }

    pub(super) fn drain(&mut self, cache: &mut HashMap<String, (String, Instant)>) -> bool {
        let mut changed = false;
        while let Ok((cwd, branch)) = self.receiver.try_recv() {
            self.pending.remove(&cwd);
            cache.insert(cwd, (branch, Instant::now()));
            changed = true;
        }
        changed
    }
}

fn probe(command: &mut ProcessCommand, timeout: Duration) -> Option<String> {
    use std::io::Read;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut bytes = Vec::new();
    child
        .stdout
        .take()?
        .take(4096)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).trim().to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn a_stalled_probe_has_a_deadline() {
        let mut command = ProcessCommand::new("sh");
        command.args(["-c", "exec sleep 5"]);
        let start = Instant::now();
        assert!(probe(&mut command, Duration::from_millis(30)).is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn branch_probe_returns_only_successful_output() {
        let mut command = ProcessCommand::new("sh");
        command.args(["-c", "printf 'main\\n'"]);
        assert_eq!(
            probe(&mut command, Duration::from_secs(1)).as_deref(),
            Some("main")
        );
        command = ProcessCommand::new("sh");
        command.args(["-c", "printf wrong; exit 1"]);
        assert!(probe(&mut command, Duration::from_secs(1)).is_none());
    }
}
