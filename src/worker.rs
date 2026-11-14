//! Background Tokio runtime. The GUI thread spawns futures here and drains finished
//! messages each frame, so it never blocks on network, disk or decoding.

use std::future::Future;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui;
use tokio::task::AbortHandle;

pub struct Worker<M> {
    rt: tokio::runtime::Runtime,
    tx: Sender<M>,
    rx: Receiver<M>,
    ctx: egui::Context,
}

impl<M: Send + 'static> Worker<M> {
    pub fn new(ctx: egui::Context) -> std::io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("kuma-worker")
            .enable_all()
            .build()?;
        let (tx, rx) = channel();
        Ok(Self { rt, tx, rx, ctx })
    }

    /// Runs `fut`; its output is delivered by [`Self::drain`]. Abort the handle to cancel.
    pub fn spawn(&self, fut: impl Future<Output = M> + Send + 'static) -> AbortHandle {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.rt
            .spawn(async move {
                let _ = tx.send(fut.await);
                ctx.request_repaint();
            })
            .abort_handle()
    }

    /// For tasks that report progress before their final message.
    pub fn notifier(&self) -> impl Fn(M) + Send + Sync + Clone + 'static {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        move |m| {
            let _ = tx.send(m);
            ctx.request_repaint();
        }
    }

    pub fn drain(&self) -> impl Iterator<Item = M> + '_ {
        self.rx.try_iter()
    }
}
