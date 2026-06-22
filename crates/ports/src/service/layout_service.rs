use crate::pty_host::{
	size_info::TerminalSizeInfo, window_metrics::TerminalWindowMetrics,
	window_size::TerminalWindowSize,
};

pub trait ILayoutService {
	fn terminal_size_info_for_window(&self, window_size: TerminalWindowSize) -> TerminalSizeInfo;

	fn terminal_size_info_for_window_metrics(
		&self,
		metrics: TerminalWindowMetrics,
	) -> TerminalSizeInfo;
}
