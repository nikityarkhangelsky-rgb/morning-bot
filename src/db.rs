use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserData {
    pub chat_id: i64,
    pub lat: f64,
    pub lon: f64,
    pub city_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub id: Option<i64>,
    pub chat_id: i64,
    /// 0 = Пн, 1 = Вт, ..., 6 = Вс
    pub day_of_week: u8,
    pub time_str: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Option<i64>,
    pub chat_id: i64,
    /// Дата в формате YYYY-MM-DD
    pub date: String,
    pub text: String,
    pub done: bool,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Database { conn };
        db.init_tables()?;
        Ok(db)
    }

    fn init_tables(&self) -> Result<()> {
        self.conn.execute_batch("
            CREATE TABLE IF NOT EXISTS users (
                chat_id INTEGER PRIMARY KEY,
                lat     REAL NOT NULL,
                lon     REAL NOT NULL,
                city_name TEXT
            );

            CREATE TABLE IF NOT EXISTS schedule (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id      INTEGER NOT NULL,
                day_of_week  INTEGER NOT NULL,
                time_str     TEXT NOT NULL,
                subject      TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tasks (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                date    TEXT NOT NULL,
                text    TEXT NOT NULL,
                done    INTEGER NOT NULL DEFAULT 0
            );
        ")?;
        Ok(())
    }

    // ──────────────────── Users ────────────────────

    pub fn upsert_user(&self, data: &UserData) -> Result<()> {
        self.conn.execute(
            "INSERT INTO users (chat_id, lat, lon, city_name)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(chat_id) DO UPDATE SET lat=?2, lon=?3, city_name=?4",
            params![data.chat_id, data.lat, data.lon, data.city_name],
        )?;
        Ok(())
    }

    pub fn get_user(&self, chat_id: i64) -> Result<Option<UserData>> {
        let mut stmt = self.conn.prepare(
            "SELECT chat_id, lat, lon, city_name FROM users WHERE chat_id = ?1"
        )?;
        let mut rows = stmt.query(params![chat_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(UserData {
                chat_id: row.get(0)?,
                lat: row.get(1)?,
                lon: row.get(2)?,
                city_name: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_all_users(&self) -> Result<Vec<UserData>> {
        let mut stmt = self.conn.prepare(
            "SELECT chat_id, lat, lon, city_name FROM users"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(UserData {
                chat_id: row.get(0)?,
                lat: row.get(1)?,
                lon: row.get(2)?,
                city_name: row.get(3)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ──────────────────── Schedule ────────────────────

    pub fn add_schedule_entry(&self, entry: &ScheduleEntry) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO schedule (chat_id, day_of_week, time_str, subject) VALUES (?1, ?2, ?3, ?4)",
            params![entry.chat_id, entry.day_of_week, entry.time_str, entry.subject],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_schedule_for_day(&self, chat_id: i64, day: u8) -> Result<Vec<ScheduleEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, chat_id, day_of_week, time_str, subject
             FROM schedule
             WHERE chat_id = ?1 AND day_of_week = ?2
             ORDER BY time_str ASC"
        )?;
        let rows = stmt.query_map(params![chat_id, day], |row| {
            Ok(ScheduleEntry {
                id: row.get(0)?,
                chat_id: row.get(1)?,
                day_of_week: row.get::<_, i64>(2)? as u8,
                time_str: row.get(3)?,
                subject: row.get(4)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn delete_schedule_entry(&self, chat_id: i64, id: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM schedule WHERE id = ?1 AND chat_id = ?2",
            params![id, chat_id],
        )?)
    }

    pub fn clear_schedule(&self, chat_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM schedule WHERE chat_id = ?1", params![chat_id])?;
        Ok(())
    }

    // ──────────────────── Tasks ────────────────────

    pub fn add_task(&self, task: &Task) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO tasks (chat_id, date, text, done) VALUES (?1, ?2, ?3, 0)",
            params![task.chat_id, task.date, task.text],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_tasks_for_date(&self, chat_id: i64, date: &str) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, chat_id, date, text, done FROM tasks WHERE chat_id = ?1 AND date = ?2"
        )?;
        let rows = stmt.query_map(params![chat_id, date], |row| {
            Ok(Task {
                id: row.get(0)?,
                chat_id: row.get(1)?,
                date: row.get(2)?,
                text: row.get(3)?,
                done: row.get::<_, i64>(4)? != 0,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn complete_task(&self, chat_id: i64, task_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET done = 1 WHERE id = ?1 AND chat_id = ?2",
            params![task_id, chat_id],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, chat_id: i64, task_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM tasks WHERE id = ?1 AND chat_id = ?2",
            params![task_id, chat_id],
        )?;
        Ok(())
    }
}
