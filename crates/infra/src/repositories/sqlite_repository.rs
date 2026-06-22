use std::{cell::RefCell, marker::PhantomData, path::Path};

use germinal_domain::aggregate_root::AggregateRoot;
use germinal_ports::repository::IRepository;
use rusqlite::{Connection, params};
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug)]
pub struct SqliteRepository<A> {
	connection: RefCell<Connection>,
	table_name: String,
	_marker:    PhantomData<A>,
}

impl<A> SqliteRepository<A>
where A: AggregateRoot + Serialize + DeserializeOwned
{
	pub fn new(path: impl AsRef<Path>, table_name: impl Into<String>) -> Result<Self, String> {
		let connection = Connection::open(path).map_err(|error| error.to_string())?;
		let table_name = table_name.into();
		connection
			.execute(
				&format!(
					"CREATE TABLE IF NOT EXISTS {table_name} (
						id INTEGER PRIMARY KEY AUTOINCREMENT,
						snapshot_json TEXT NOT NULL
					)"
				),
				[],
			)
			.map_err(|error| error.to_string())?;

		Ok(Self { connection: RefCell::new(connection), table_name, _marker: PhantomData })
	}

	fn deserialize(&self, snapshot_json: String) -> Result<A, String> {
		serde_json::from_str(&snapshot_json).map_err(|error| error.to_string())
	}
}

impl<A> IRepository for SqliteRepository<A>
where A: AggregateRoot + Serialize + DeserializeOwned
{
	type Aggregate = A;
	type Id = u64;

	fn get(&self, id: Self::Id) -> Result<Option<Self::Aggregate>, String> {
		let connection = self.connection.borrow();
		let mut statement = connection
			.prepare(&format!("SELECT snapshot_json FROM {} WHERE id = ?1", self.table_name))
			.map_err(|error| error.to_string())?;
		let mut rows = statement.query(params![id]).map_err(|error| error.to_string())?;
		let Some(row) = rows.next().map_err(|error| error.to_string())? else {
			return Ok(None);
		};
		let snapshot_json = row.get::<_, String>(0).map_err(|error| error.to_string())?;
		self.deserialize(snapshot_json).map(Some)
	}

	fn list(&self) -> Result<Vec<(Self::Id, Self::Aggregate)>, String> {
		let connection = self.connection.borrow();
		let mut statement = connection
			.prepare(&format!("SELECT id, snapshot_json FROM {} ORDER BY id", self.table_name))
			.map_err(|error| error.to_string())?;
		let rows = statement
			.query_map([], |row| {
				let id = row.get::<_, u64>(0)?;
				let snapshot_json = row.get::<_, String>(1)?;
				Ok((id, snapshot_json))
			})
			.map_err(|error| error.to_string())?;

		rows
			.map(|row| {
				let (id, snapshot_json) = row.map_err(|error| error.to_string())?;
				let aggregate = self.deserialize(snapshot_json)?;
				Ok((id, aggregate))
			})
			.collect()
	}

	fn insert(&self, aggregate: Self::Aggregate) -> Result<Self::Id, String> {
		let snapshot_json = serde_json::to_string(&aggregate).map_err(|error| error.to_string())?;
		self
			.connection
			.borrow()
			.execute(&format!("INSERT INTO {} (snapshot_json) VALUES (?1)", self.table_name), params![
				snapshot_json
			])
			.map_err(|error| error.to_string())?;
		Ok(self.connection.borrow().last_insert_rowid() as u64)
	}

	fn update(&self, id: Self::Id, aggregate: Self::Aggregate) -> Result<(), String> {
		let snapshot_json = serde_json::to_string(&aggregate).map_err(|error| error.to_string())?;
		self
			.connection
			.borrow()
			.execute(
				&format!("UPDATE {} SET snapshot_json = ?1 WHERE id = ?2", self.table_name),
				params![snapshot_json, id],
			)
			.map(|_| ())
			.map_err(|error| error.to_string())
	}

	fn delete(&self, id: Self::Id) -> Result<(), String> {
		self
			.connection
			.borrow()
			.execute(&format!("DELETE FROM {} WHERE id = ?1", self.table_name), params![id])
			.map(|_| ())
			.map_err(|error| error.to_string())
	}
}
