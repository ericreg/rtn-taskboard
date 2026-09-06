use anyhow::{Context, ensure};
use rtn_mq::{HostStorage, Identity};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{path::Path, sync::Mutex, time::Duration};

// A dedicated synchronous connection matches rtn-mq's persistence callbacks and
// avoids blocking on the application's asynchronous SQLx pool inside the runtime.
pub(super) struct DatabaseStorage {
    connection: Mutex<(Connection, i64)>,
    initial_state: Vec<u8>,
    private_key: [u8; 32],
}

impl DatabaseStorage {
    pub(super) fn open(path: &Path) -> anyhow::Result<(Identity, Self)> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .context("open rtn-mq database storage")?;
        connection.busy_timeout(Duration::from_secs(15))?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let (key, initial_state, revision): (Vec<u8>, Vec<u8>, i64) = connection
            .query_row(
                "SELECT private_key,host_state,revision FROM rtn_identity WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .context("Backend is not seeded. Run taskboard seed EMAIL [NAME] with the password on stdin first.")?;
        let private_key: [u8; 32] = key
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid backend private key length"))?;
        ensure!(
            !initial_state.is_empty(),
            "Backend enrollment state is empty"
        );
        Ok((
            Identity::from_bytes(&private_key),
            Self {
                connection: Mutex::new((connection, revision)),
                initial_state,
                private_key,
            },
        ))
    }
}

impl HostStorage for DatabaseStorage {
    fn load(&self) -> rtn_mq::Result<Option<Vec<u8>>> {
        Ok(Some(self.initial_state.clone()))
    }

    fn save(&self, bytes: &[u8]) -> rtn_mq::Result<()> {
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| rtn_mq::Error::Io("rtn-mq database lock poisoned".into()))?;
        let (connection, revision) = &mut *guard;
        // Fail on competing hosts instead of silently overwriting grants or consumed uses.
        let changed = connection.execute(
            "UPDATE rtn_identity SET host_state=?1,revision=revision+1 WHERE id=1 AND revision=?2 AND private_key=?3",
            rusqlite::params![bytes, *revision, self.private_key.as_slice()],
        ).map_err(|error| rtn_mq::Error::Io(format!("save rtn-mq database state: {error}")))?;
        if changed != 1 {
            return Err(rtn_mq::Error::Io(
                "Backend identity changed in another process; stop competing backends and restart"
                    .into(),
            ));
        }
        *revision += 1;
        Ok(())
    }
}
