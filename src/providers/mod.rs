use crate::model::Message;

pub mod streamx;
pub mod twitch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

impl ConnectionStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Connecting => "подключение",
            Self::Connected => "онлайн",
            Self::Reconnecting => "переподключение",
            Self::Failed => "ошибка",
        }
    }

    pub fn color(&self) -> egui::Color32 {
        match self {
            Self::Connecting => egui::Color32::from_rgb(0xE0, 0xB0, 0x4C),
            Self::Connected => egui::Color32::from_rgb(0x3D, 0xD6, 0x8C),
            Self::Reconnecting => egui::Color32::from_rgb(0xE0, 0xB0, 0x4C),
            Self::Failed => egui::Color32::from_rgb(0xE0, 0x5C, 0x5C),
        }
    }
}

#[derive(Debug)]
pub enum ProviderEvent {
    Status(usize, ConnectionStatus),
    Message(usize, Message),
    History(usize, Vec<Message>),
    Note(usize, String),
    RoomId(usize, String),
    Error(usize, String),
}
