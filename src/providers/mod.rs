use crate::model::Message;

pub mod streamx;
pub mod twitch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    /// Socket is up, but the backfill is still in flight.
    Syncing,
    Connected,
    Reconnecting,
    Failed,
}

impl ConnectionStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Connecting => "подключение",
            Self::Syncing => "история",
            Self::Connected => "онлайн",
            Self::Reconnecting => "переподключение",
            Self::Failed => "ошибка",
        }
    }

    pub fn color(&self) -> egui::Color32 {
        match self {
            Self::Connecting | Self::Syncing | Self::Reconnecting => {
                egui::Color32::from_rgb(0xE0, 0xB0, 0x4C)
            }
            Self::Connected => egui::Color32::from_rgb(0x3D, 0xD6, 0x8C),
            Self::Failed => egui::Color32::from_rgb(0xE0, 0x5C, 0x5C),
        }
    }
}

#[derive(Debug)]
pub enum ProviderEvent {
    Status(usize, ConnectionStatus),
    Message(Message),
    History(Vec<Message>),
    Note(String),
    Error(usize, String),
}

/// Detached work that belongs to one connection attempt (the history backfill).
/// Dropping it aborts the work, so a slow backfill cannot report status for the
/// next session. Messages already queued by it are harmless: the timeline
/// dedupes by id and sorts by timestamp.
pub struct AbortOnDrop(pub tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn syncing_status_shows_history_loading() {
        assert_eq!(ConnectionStatus::Syncing.label(), "история");
        assert_eq!(
            ConnectionStatus::Syncing.color(),
            ConnectionStatus::Connecting.color()
        );
        assert_ne!(
            ConnectionStatus::Syncing.color(),
            ConnectionStatus::Connected.color()
        );
    }

    #[tokio::test]
    async fn background_job_is_cancelled_with_its_session() {
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let job = AbortOnDrop(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            flag.store(true, Ordering::Relaxed);
        }));

        drop(job);
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(!ran.load(Ordering::Relaxed), "задача пережила свою сессию");
    }
}
