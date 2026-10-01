//! Background loader for the selected session's Ledger run view, so the
//! `ledger run show` calls stay off the render loop.

use crate::ledger_run::LedgerRunView;
use crate::session::Instance;
use crate::tui::worker::Worker;

pub struct LedgerPoller {
    worker: Worker<Instance, (String, Option<LedgerRunView>)>,
}

impl LedgerPoller {
    pub fn new() -> Self {
        Self {
            worker: Worker::spawn("aoe-ledger-poller", |instance: Instance| {
                let view = crate::ledger_run::load_view(&instance);
                (instance.id, view)
            }),
        }
    }

    pub fn request_refresh(&self, instance: Instance) {
        self.worker.request(instance);
    }

    pub fn try_recv_updates(
        &self,
    ) -> Result<(String, Option<LedgerRunView>), std::sync::mpsc::TryRecvError> {
        self.worker.try_recv()
    }
}

impl Default for LedgerPoller {
    fn default() -> Self {
        Self::new()
    }
}
