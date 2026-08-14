use germinal_ports::{
    pty_host::{
        profile::TerminalProfile, scale_factor::TerminalScaleFactor, size_info::TerminalSizeInfo,
        window_metrics::TerminalWindowMetrics, window_size::TerminalWindowSize,
    },
    service::layout_service::ILayoutService,
};

#[derive(kudi::DepInj)]
#[target(LayoutService)]
pub struct LayoutServiceState {
    profile: TerminalProfile,
}

impl LayoutServiceState {
    pub fn new(profile: TerminalProfile) -> Self {
        Self { profile }
    }
}

impl Default for LayoutServiceState {
    fn default() -> Self {
        Self::new(TerminalProfile::DEFAULT)
    }
}

impl<Deps> LayoutService<Deps>
where
    Deps: AsRef<LayoutServiceState>,
{
    fn size_info_for_window(&self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
        self.size_info_for_window_metrics(TerminalWindowMetrics::new(
            window_size,
            TerminalScaleFactor::DEFAULT,
        ))
    }

    fn size_info_for_window_metrics(&self, metrics: TerminalWindowMetrics) -> TerminalSizeInfo {
        let state: &LayoutServiceState = self.prj_ref().as_ref();
        state.profile.size_info_for_window_metrics(metrics)
    }
}

impl<Deps> ILayoutService for LayoutService<Deps>
where
    Deps: AsRef<LayoutServiceState>,
{
    fn terminal_size_info_for_window(&self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
        self.size_info_for_window(window_size)
    }

    fn terminal_size_info_for_window_metrics(
        &self,
        metrics: TerminalWindowMetrics,
    ) -> TerminalSizeInfo {
        self.size_info_for_window_metrics(metrics)
    }
}
