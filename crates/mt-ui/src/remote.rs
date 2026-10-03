//! Commands from outside the window. A second launch of the app hands its
//! `--run` commands to the running one instead of opening another window (and
//! polling MISO twice). The binary owns the transport; this is the UI's end.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};

/// The sending end: cheap to clone, usable from any thread.
#[derive(Clone)]
pub struct Remote {
    tx: Sender<Vec<String>>,
    ctx: Arc<OnceLock<egui::Context>>,
}

/// The receiving end, handed to the app through `Deps`.
pub struct RemoteInbox {
    rx: Receiver<Vec<String>>,
    ctx: Arc<OnceLock<egui::Context>>,
}

pub fn channel_pair() -> (Remote, RemoteInbox) {
    let (tx, rx) = channel();
    let ctx = Arc::new(OnceLock::new());
    (
        Remote {
            tx,
            ctx: ctx.clone(),
        },
        RemoteInbox { rx, ctx },
    )
}

impl Remote {
    /// Queue commands (none just brings the window forward) and wake the UI.
    /// False once the app has gone.
    pub fn send(&self, commands: Vec<String>) -> bool {
        let sent = self.tx.send(commands).is_ok();
        if let Some(ctx) = self.ctx.get() {
            ctx.request_repaint();
        }
        sent
    }
}

impl RemoteInbox {
    /// Let senders wake this context. Messages sent earlier wait in the queue.
    pub(crate) fn attach(&self, ctx: &egui::Context) {
        let _ = self.ctx.set(ctx.clone());
    }

    /// Everything received since the last call; `None` if nothing arrived.
    pub(crate) fn drain(&self) -> Option<Vec<String>> {
        let mut got: Option<Vec<String>> = None;
        while let Ok(commands) = self.rx.try_recv() {
            got.get_or_insert_with(Vec::new).extend(commands);
        }
        got
    }

    /// Wait up to `timeout` for a message, then take everything queued.
    /// Empty if nothing came. For tests and tools.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Vec<String> {
        let mut out = self.rx.recv_timeout(timeout).unwrap_or_default();
        out.extend(self.drain().unwrap_or_default());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_queue_until_drained() {
        let (remote, inbox) = channel_pair();
        assert!(inbox.drain().is_none());
        assert!(remote.send(vec![]));
        assert_eq!(inbox.drain(), Some(vec![]), "an empty message still counts");
        remote.send(vec!["GP MINN.HUB".into()]);
        remote.send(vec!["MAP".into(), "LMP".into()]);
        assert_eq!(
            inbox.drain(),
            Some(vec!["GP MINN.HUB".into(), "MAP".into(), "LMP".into()])
        );
        drop(inbox);
        assert!(!remote.send(vec![]), "the app has gone");
    }
}
