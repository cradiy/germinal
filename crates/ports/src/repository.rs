use germinal_domain::aggregate_root::AggregateRoot;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("repository persistence is unavailable")]
    PersistenceUnavailable,
    #[error("repository data is inconsistent")]
    InconsistentData,
}

pub type RepositoryResult<T> = Result<T, RepositoryError>;
pub type RepositoryEntry<Id, Aggregate> = (Id, Aggregate);

pub trait IRepository {
    type Id: Copy + Eq;
    type Aggregate: AggregateRoot;

    fn get(&self, id: Self::Id) -> RepositoryResult<Option<Self::Aggregate>>;
    fn list(&self) -> RepositoryResult<Vec<RepositoryEntry<Self::Id, Self::Aggregate>>>;
    fn insert(&self, aggregate: Self::Aggregate) -> RepositoryResult<Self::Id>;
    fn update(&self, id: Self::Id, aggregate: Self::Aggregate) -> RepositoryResult<()>;
    fn delete(&self, id: Self::Id) -> RepositoryResult<()>;
}
