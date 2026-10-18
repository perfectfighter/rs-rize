use crate::{ApiError, ApiResult};
use rusqlite::Connection;
use std::path::Path;

pub fn open(location: &str) -> ApiResult<Connection> {
    if location != ":memory:" {
        if let Some(parent) = Path::new(location).parent() {
            if !parent.as_os_str().is_empty() {
                let mut builder = std::fs::DirBuilder::new();
                builder.recursive(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(parent).map_err(|_| ApiError::internal())?;
            }
        }
    }
    let db = Connection::open(location)?;
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;
    CREATE TABLE IF NOT EXISTS users(id TEXT PRIMARY KEY,email TEXT NOT NULL UNIQUE,name TEXT NOT NULL,password_hash TEXT NOT NULL,created_at INTEGER NOT NULL) STRICT;
    CREATE TABLE IF NOT EXISTS sessions(token_hash TEXT PRIMARY KEY,user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,expires_at INTEGER NOT NULL,created_at INTEGER NOT NULL) STRICT;
    CREATE TABLE IF NOT EXISTS teams(id TEXT PRIMARY KEY,name TEXT NOT NULL,created_at INTEGER NOT NULL) STRICT;
    CREATE TABLE IF NOT EXISTS members(team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,role TEXT NOT NULL CHECK(role IN ('owner','admin','member','viewer')),finance INTEGER NOT NULL DEFAULT 0,resourcing INTEGER NOT NULL DEFAULT 0,joined_at INTEGER NOT NULL,PRIMARY KEY(team_id,user_id)) STRICT;
    CREATE TABLE IF NOT EXISTS invites(code_hash TEXT PRIMARY KEY,team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,email TEXT NOT NULL,role TEXT NOT NULL,inviter_id TEXT NOT NULL REFERENCES users(id),expires_at INTEGER NOT NULL,used_at INTEGER) STRICT;
    CREATE TABLE IF NOT EXISTS records(team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,kind TEXT NOT NULL,id TEXT NOT NULL,owner_id TEXT NOT NULL REFERENCES users(id),data TEXT NOT NULL,version INTEGER NOT NULL,deleted INTEGER NOT NULL DEFAULT 0,updated_at INTEGER NOT NULL,PRIMARY KEY(team_id,kind,id)) STRICT;
    CREATE TABLE IF NOT EXISTS events(cursor INTEGER PRIMARY KEY AUTOINCREMENT,team_id TEXT NOT NULL,kind TEXT NOT NULL,id TEXT NOT NULL,owner_id TEXT NOT NULL,data TEXT NOT NULL,version INTEGER NOT NULL,deleted INTEGER NOT NULL,updated_at INTEGER NOT NULL) STRICT;
    CREATE INDEX IF NOT EXISTS events_team_cursor ON events(team_id,cursor); CREATE INDEX IF NOT EXISTS sessions_expiry ON sessions(expires_at);
    CREATE TABLE IF NOT EXISTS kv(team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,namespace TEXT NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(team_id,namespace,key)) STRICT;")?;
    #[cfg(unix)]
    if location != ":memory:" {
        use std::os::unix::fs::PermissionsExt;
        for suffix in ["", "-wal", "-shm"] {
            let file = format!("{location}{suffix}");
            match std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err(ApiError::internal()),
            }
        }
    }
    Ok(db)
}
pub fn transaction<T>(db: &Connection, op: impl FnOnce() -> ApiResult<T>) -> ApiResult<T> {
    db.execute_batch("BEGIN IMMEDIATE")?;
    match op() {
        Ok(value) => {
            if let Err(e) = db.execute_batch("COMMIT") {
                let _ = db.execute_batch("ROLLBACK");
                return Err(e.into());
            }
            Ok(value)
        }
        Err(e) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
