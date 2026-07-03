use germinal_domain::aggregate_root::AggregateRoot;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepositoryError {
	#[error("repository persistence is unavailable")]
	PersistenceUnavailable,
	#[error("repository data is inconsistent")]
	InconsistentData,
}

pub trait IRepository {
	type Id: Copy + Eq;
	type Aggregate: AggregateRoot;

	fn get(&self, id: Self::Id) -> Result<Option<Self::Aggregate>, RepositoryError>;
	fn list(&self) -> Result<Vec<(Self::Id, Self::Aggregate)>, RepositoryError>;
	fn insert(&self, aggregate: Self::Aggregate) -> Result<Self::Id, RepositoryError>;
	fn update(&self, id: Self::Id, aggregate: Self::Aggregate) -> Result<(), RepositoryError>;
	fn delete(&self, id: Self::Id) -> Result<(), RepositoryError>;
}
