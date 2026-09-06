use crate::db::{self, Database};
use anyhow::{Context, ensure};
use rtn_mq::{HostStorage, Identity};
use std::sync::Mutex;
use turso::Connection;

// The endpoint's synchronous HostStorage callbacks use a dedicated Turso connection.
// All connections share the application's database handle and WAL coordination.
pub(super) struct DatabaseStorage {
    connection: Mutex<(Connection, i64)>,
    initial_state: Vec<u8>,
    private_key: [u8; 32],
}

impl DatabaseStorage {
    pub(super) async fn open(database: &Database) -> anyhow::Result<(Identity, Self)> {
        let connection = database
            .connect()
            .await
            .context("open rtn-mq database storage")?;
        let row = db::optional(&connection, "SELECT private_key,host_state,revision FROM rtn_identity WHERE id=1", ())
            .await?.context("Backend is not seeded. Run taskboard seed EMAIL [NAME] with the password on stdin first.")?;
        let key: Vec<u8> = row.try_get("private_key")?;
        let initial_state: Vec<u8> = row.try_get("host_state")?;
        let revision: i64 = row.try_get("revision")?;
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
        let persist = || {
            futures_executor::block_on(connection.execute(
            "UPDATE rtn_identity SET host_state=?1,revision=revision+1 WHERE id=1 AND revision=?2 AND private_key=?3",
            turso::params![bytes, *revision, self.private_key.as_slice()],
        ))
        };
        // A contending application writer must be able to finish even when the
        // backend has just one Tokio worker. HostStorage itself is synchronous.
        let changed = if tokio::runtime::Handle::try_current().is_ok_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        }) {
            tokio::task::block_in_place(persist)
        } else {
            persist()
        }
        .map_err(|error| rtn_mq::Error::Io(format!("save rtn-mq database state: {error}")))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn enrollment_save_allows_a_contending_writer_to_finish() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(&directory.path().join("taskboard.db"))
            .await
            .unwrap();
        database
            .connect()
            .await
            .unwrap()
            .execute(
                "INSERT INTO rtn_identity(id,private_key,host_state) VALUES(1,?,?)",
                turso::params![
                    Identity::generate().to_bytes().as_slice(),
                    rtn_mq::generate_host_state()
                ],
            )
            .await
            .unwrap();
        let (_, storage) = DatabaseStorage::open(&database).await.unwrap();
        let snapshot = storage.load().unwrap().unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn(async move {
            let mut conn = database.connect().await.unwrap();
            let tx = conn
                .transaction_with_behavior(turso::transaction::TransactionBehavior::Immediate)
                .await
                .unwrap();
            tx.execute(
                "INSERT INTO app_settings VALUES('writer_test','committed')",
                (),
            )
            .await
            .unwrap();
            ready.send(()).unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            tx.commit().await.unwrap();
        });
        started.await.unwrap();
        let save = tokio::spawn(async move {
            storage.save(&snapshot).unwrap();
        });
        tokio::time::timeout(Duration::from_secs(3), save)
            .await
            .unwrap()
            .unwrap();
        writer.await.unwrap();
    }
}
