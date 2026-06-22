use germinal_domain::gshell::vo::gshell_id::GShellId;

pub trait IGNativeService {
	fn ensure_gshell_gnative(&self, gshell_id: GShellId);
}
