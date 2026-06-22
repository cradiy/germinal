use germinal_domain::aggregate_root::AggregateRoot;

pub trait IRepository {
	type Id: Copy + Eq;
	type Aggregate: AggregateRoot;

	fn get(&self, id: Self::Id) -> Result<Option<Self::Aggregate>, String>;
	fn list(&self) -> Result<Vec<(Self::Id, Self::Aggregate)>, String>;
	fn insert(&self, aggregate: Self::Aggregate) -> Result<Self::Id, String>;
	fn update(&self, id: Self::Id, aggregate: Self::Aggregate) -> Result<(), String>;
	fn delete(&self, id: Self::Id) -> Result<(), String>;
}
