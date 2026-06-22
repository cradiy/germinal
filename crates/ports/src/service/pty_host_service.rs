use germinal_domain::pty_host::entity::pty_host::PtyHost;

use crate::repository::IRepository;

pub trait IPtyHostRuntimeRepositoryProvider {
	type PtyHostRuntimeRepository: IRepository<Id = u64, Aggregate = PtyHost>;

	fn pty_host_runtime_repository(&self) -> &Self::PtyHostRuntimeRepository;
}
