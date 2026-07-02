use germinal_domain::aggregate_root::AggregateRoot;

use crate::error::BoxResult;

pub trait IRepository {
	type Id: Copy + Eq;
	type Aggregate: AggregateRoot;

	fn get(&self, id: Self::Id) -> BoxResult<Option<Self::Aggregate>>;
	fn list(&self) -> BoxResult<Vec<(Self::Id, Self::Aggregate)>>;
	fn insert(&self, aggregate: Self::Aggregate) -> BoxResult<Self::Id>;
	fn update(&self, id: Self::Id, aggregate: Self::Aggregate) -> BoxResult<()>;
	fn delete(&self, id: Self::Id) -> BoxResult<()>;
}
