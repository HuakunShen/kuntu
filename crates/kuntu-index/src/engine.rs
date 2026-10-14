//! Owned-row adapter: synchronous SQLite behind the existing awaitable API.
use super::{IndexError, Result};
use rusqlite::{
  types::{FromSql, Value},
  Params,
};
use std::cell::Cell;
#[cfg(test)]
thread_local! { pub(crate) static OWNED_ROW_COPIES: Cell<usize> = const { Cell::new(0) }; }
#[derive(Debug)]
pub struct Connection {
  pub raw: rusqlite::Connection,
  pub readonly: Cell<bool>,
  pub minor: i64,
  pub lease: std::cell::RefCell<Option<kuntu_store::Lease>>,
}
pub struct Rows {
  rows: std::vec::IntoIter<Row>,
}
pub struct Row {
  values: Vec<Value>,
}
impl Row {
  pub fn get<T: FromSql>(&self, n: usize) -> rusqlite::Result<T> {
    T::column_result(rusqlite::types::ValueRef::from(&self.values[n])).map_err(|e| {
      rusqlite::Error::FromSqlConversionFailure(n, self.values[n].data_type(), Box::new(e))
    })
  }
}
impl Rows {
  pub async fn next(&mut self) -> rusqlite::Result<Option<Row>> {
    Ok(self.rows.next())
  }
}
impl Connection {
  pub async fn query(&self, sql: &str, p: impl Params) -> rusqlite::Result<Rows> {
    let mut s = self.raw.prepare_cached(sql)?;
    let n = s.column_count();
    let rows = s
      .query_map(p, |r| {
        #[cfg(test)]
        OWNED_ROW_COPIES.with(|n| n.set(n.get() + 1));
        let mut v = Vec::with_capacity(n);
        for i in 0..n {
          v.push(r.get(i)?);
        }
        Ok(Row { values: v })
      })?
      .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Rows {
      rows: rows.into_iter(),
    })
  }
  pub async fn execute(&self, sql: &str, p: impl Params) -> rusqlite::Result<usize> {
    self.raw.prepare_cached(sql)?.execute(p)
  }
  pub async fn transaction(&self) -> Result<Transaction<'_>> {
    self.begin_writer_txn(false)
  }
  pub fn begin_writer_txn(&self, exclusive: bool) -> Result<Transaction<'_>> {
    if self.readonly.get() {
      return Err(IndexError::ReadOnly);
    }
    self.raw.execute_batch(if exclusive {
      "BEGIN EXCLUSIVE"
    } else {
      "BEGIN IMMEDIATE"
    })?;
    let tx = Transaction {
      conn: self,
      done: false,
    };
    let min = self.raw.query_row(
      "SELECT value FROM meta WHERE key='min_writer_minor'",
      [],
      |r| r.get::<_, i64>(0),
    );
    match min {
      Ok(m) if m > self.minor => {
        self.readonly.set(true);
        self.lease.borrow_mut().take();
        Err(IndexError::ReadOnly)
      }
      Ok(_) => Ok(tx),
      Err(e) => {
        let tables: i64 = self.raw.query_row(
          "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
          [],
          |r| r.get(0),
        )?;
        if tables == 0 && exclusive {
          Ok(tx)
        } else {
          Err(e.into())
        }
      }
    }
  }
}
pub struct Transaction<'a> {
  pub conn: &'a Connection,
  done: bool,
}
impl std::ops::Deref for Transaction<'_> {
  type Target = Connection;
  fn deref(&self) -> &Connection {
    self.conn
  }
}
impl Transaction<'_> {
  pub async fn commit(mut self) -> Result<()> {
    self.conn.raw.execute(
      "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='generation'",
      [],
    )?;
    self.conn.raw.execute_batch("COMMIT")?;
    self.done = true;
    Ok(())
  }
}
impl Drop for Transaction<'_> {
  fn drop(&mut self) {
    if !self.done {
      let _ = self.conn.raw.execute_batch("ROLLBACK");
    }
  }
}
