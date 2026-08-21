#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalNotificationOccasion {
    Always,
    Unfocused,
    Invisible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalNotification {
    pub title: Option<String>,
    pub body: Option<String>,
    pub occasion: TerminalNotificationOccasion,
    pub focus_on_activation: bool,
}

impl TerminalNotification {
    pub fn new(
        title: Option<String>,
        body: Option<String>,
        occasion: TerminalNotificationOccasion,
    ) -> Self {
        Self {
            title,
            body,
            occasion,
            focus_on_activation: true,
        }
    }
}
