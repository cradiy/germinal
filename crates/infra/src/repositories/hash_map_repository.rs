use std::{
	cell::{Cell, RefCell},
	collections::HashMap,
	marker::PhantomData,
};

use germinal_domain::aggregate_root::AggregateRoot;
use germinal_ports::repository::IRepository;
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug)]
pub struct HashMapRepository<A> {
	snapshots: RefCell<HashMap<u64, String>>,
	next_id:   Cell<u64>,
	_marker:   PhantomData<A>,
}

impl<A> HashMapRepository<A>
where A: AggregateRoot + Serialize + DeserializeOwned
{
	pub fn new() -> Self {
		Self {
			snapshots: RefCell::new(HashMap::new()),
			next_id:   Cell::new(0),
			_marker:   PhantomData,
		}
	}

	fn serialize(&self, aggregate: &A) -> Result<String, String> {
		serde_json::to_string(aggregate).map_err(|error| error.to_string())
	}

	fn deserialize(&self, snapshot_json: &str) -> Result<A, String> {
		serde_json::from_str(snapshot_json).map_err(|error| error.to_string())
	}
}

impl<A> Default for HashMapRepository<A>
where A: AggregateRoot + Serialize + DeserializeOwned
{
	fn default() -> Self { Self::new() }
}

impl<A> IRepository for HashMapRepository<A>
where A: AggregateRoot + Serialize + DeserializeOwned
{
	type Aggregate = A;
	type Id = u64;

	fn get(&self, id: Self::Id) -> Result<Option<Self::Aggregate>, String> {
		self
			.snapshots
			.borrow()
			.get(&id)
			.map(|snapshot_json| self.deserialize(snapshot_json))
			.transpose()
	}

	fn list(&self) -> Result<Vec<(Self::Id, Self::Aggregate)>, String> {
		self
			.snapshots
			.borrow()
			.iter()
			.map(|(id, snapshot_json)| self.deserialize(snapshot_json).map(|aggregate| (*id, aggregate)))
			.collect()
	}

	fn insert(&self, aggregate: Self::Aggregate) -> Result<Self::Id, String> {
		let id = self.next_id.get();
		self.next_id.set(id + 1);
		let snapshot_json = self.serialize(&aggregate)?;
		self.snapshots.borrow_mut().insert(id, snapshot_json);
		Ok(id)
	}

	fn update(&self, id: Self::Id, aggregate: Self::Aggregate) -> Result<(), String> {
		let snapshot_json = self.serialize(&aggregate)?;
		self.snapshots.borrow_mut().insert(id, snapshot_json);
		Ok(())
	}

	fn delete(&self, id: Self::Id) -> Result<(), String> {
		self.snapshots.borrow_mut().remove(&id);
		Ok(())
	}
}
