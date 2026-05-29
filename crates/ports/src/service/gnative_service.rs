use germinal_domain::workspace::pane_id::PaneId;

pub trait IGNativeService {
	fn ensure_pane_gnative(&self, pane_id: PaneId);
}
