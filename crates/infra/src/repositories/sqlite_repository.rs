use std::{cell::RefCell, marker::PhantomData, path::Path};

use germinal_domain::aggregate_root::AggregateRoot;
use germinal_ports::{error::BoxResult, repository::IRepository};
use rusqlite::{Connection, params};
use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

#[derive(Debug, Error)]
enum SqliteRepositoryError {
	#[error("failed to open sqlite database at {path}: {source}")]
	OpenConnection {
		path:   String,
		#[source]
		source: rusqlite::Error,
	},
	#[error("failed to initialize sqlite table {table_name}: {source}")]
	CreateTable {
		table_name: String,
		#[source]
		source:     rusqlite::Error,
	},
	#[error("failed to deserialize repository snapshot: {0}")]
	DeserializeSnapshot(#[source] serde_json::Error),
	#[error("failed to serialize repository snapshot: {0}")]
	SerializeSnapshot(#[source] serde_json::Error),
	#[error("sqlite query failed for table {table_name}: {source}")]
	Query {
		table_name: String,
		#[source]
		source:     rusqlite::Error,
	},
}

#[derive(Debug)]
pub struct SqliteRepository<A> {
	connection: RefCell<Connection>,
	table_name: String,
	_marker:    PhantomData<A>,
}

impl<A> SqliteRepository<A>
where A: AggregateRoot + Serialize + DeserializeOwned
{
	pub fn new(path: impl AsRef<Path>, table_name: impl Into<String>) -> BoxResult<Self> {
		let path = path.as_ref();
		let path_display = path.display().to_string();
		let connection = Connection::open(path)
			.map_err(|source| SqliteRepositoryError::OpenConnection { path: path_display, source })?;
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
			.map_err(|source| SqliteRepositoryError::CreateTable {
				table_name: table_name.clone(),
				source,
			})?;

		Ok(Self { connection: RefCell::new(connection), table_name, _marker: PhantomData })
	}

	fn deserialize(&self, snapshot_json: String) -> BoxResult<A> {
		serde_json::from_str(&snapshot_json)
			.map_err(|source| SqliteRepositoryError::DeserializeSnapshot(source).into())
	}
}

impl<A> IRepository for SqliteRepository<A>
where A: AggregateRoot + Serialize + DeserializeOwned
{
	type Aggregate = A;
	type Id = u64;

	fn get(&self, id: Self::Id) -> BoxResult<Option<Self::Aggregate>> {
		let connection = self.connection.borrow();
		let mut statement = connection
			.prepare(&format!("SELECT snapshot_json FROM {} WHERE id = ?1", self.table_name))
			.map_err(|source| SqliteRepositoryError::Query {
				table_name: self.table_name.clone(),
				source,
			})?;
		let mut rows = statement.query(params![id]).map_err(|source| SqliteRepositoryError::Query {
			table_name: self.table_name.clone(),
			source,
		})?;
		let Some(row) = rows.next().map_err(|source| SqliteRepositoryError::Query {
			table_name: self.table_name.clone(),
			source,
		})?
		else {
			return Ok(None);
		};
		let snapshot_json = row.get::<_, String>(0).map_err(|source| SqliteRepositoryError::Query {
			table_name: self.table_name.clone(),
			source,
		})?;
		self.deserialize(snapshot_json).map(Some)
	}

	fn list(&self) -> BoxResult<Vec<(Self::Id, Self::Aggregate)>> {
		let connection = self.connection.borrow();
		let mut statement = connection
			.prepare(&format!("SELECT id, snapshot_json FROM {} ORDER BY id", self.table_name))
			.map_err(|source| SqliteRepositoryError::Query {
				table_name: self.table_name.clone(),
				source,
			})?;
		let rows = statement
			.query_map([], |row| {
				let id = row.get::<_, u64>(0)?;
				let snapshot_json = row.get::<_, String>(1)?;
				Ok((id, snapshot_json))
			})
			.map_err(|source| SqliteRepositoryError::Query {
				table_name: self.table_name.clone(),
				source,
			})?;

		rows
			.map(|row| {
				let (id, snapshot_json) = row.map_err(|source| SqliteRepositoryError::Query {
					table_name: self.table_name.clone(),
					source,
				})?;
				let aggregate = self.deserialize(snapshot_json)?;
				Ok((id, aggregate))
			})
			.collect()
	}

	fn insert(&self, aggregate: Self::Aggregate) -> BoxResult<Self::Id> {
		let snapshot_json =
			serde_json::to_string(&aggregate).map_err(SqliteRepositoryError::SerializeSnapshot)?;
		self
			.connection
			.borrow()
			.execute(&format!("INSERT INTO {} (snapshot_json) VALUES (?1)", self.table_name), params![
				snapshot_json
			])
			.map_err(|source| SqliteRepositoryError::Query {
				table_name: self.table_name.clone(),
				source,
			})?;
		Ok(self.connection.borrow().last_insert_rowid() as u64)
	}

	fn update(&self, id: Self::Id, aggregate: Self::Aggregate) -> BoxResult<()> {
		let snapshot_json =
			serde_json::to_string(&aggregate).map_err(SqliteRepositoryError::SerializeSnapshot)?;
		self
			.connection
			.borrow()
			.execute(
				&format!("UPDATE {} SET snapshot_json = ?1 WHERE id = ?2", self.table_name),
				params![snapshot_json, id],
			)
			.map(|_| ())
			.map_err(|source| {
				SqliteRepositoryError::Query { table_name: self.table_name.clone(), source }.into()
			})
	}

	fn delete(&self, id: Self::Id) -> BoxResult<()> {
		self
			.connection
			.borrow()
			.execute(&format!("DELETE FROM {} WHERE id = ?1", self.table_name), params![id])
			.map(|_| ())
			.map_err(|source| {
				SqliteRepositoryError::Query { table_name: self.table_name.clone(), source }.into()
			})
	}
}
