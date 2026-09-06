CREATE TABLE rtn_identity (
 id INTEGER PRIMARY KEY CHECK(id = 1),
 private_key BLOB NOT NULL CHECK(typeof(private_key) = 'blob' AND length(private_key) = 32),
 host_state BLOB NOT NULL CHECK(typeof(host_state) = 'blob' AND length(host_state) BETWEEN 1 AND 4194304),
 revision INTEGER NOT NULL DEFAULT 0
);
